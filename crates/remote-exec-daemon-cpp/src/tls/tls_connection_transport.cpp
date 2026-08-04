#include "tls/tls_connection_transport.h"

#include <algorithm>
#include <climits>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

#include "platform/basic_mutex.h"
#include "platform/platform.h"
#include "port_forward/port_forward_socket_ops.h"
#include "runtime/daemon_thread.h"
#include "tls/openssl_compat.h"

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
#include <openssl/pem.h>
#include <openssl/x509.h>

namespace {

const std::size_t TLS_IO_BUFFER_SIZE = 16U * 1024U;
const std::size_t TLS_MAX_BUFFERED_PLAINTEXT = 4U * 1024U * 1024U;
const unsigned long TLS_PUMP_WAIT_MS = 25UL;

std::vector<unsigned char> certificate_der(X509* certificate) {
    const int size = i2d_X509(certificate, nullptr);
    if (size <= 0) {
        throw std::runtime_error(openssl_compat::error_string("encoding certificate"));
    }
    std::vector<unsigned char> der(static_cast<std::size_t>(size));
    unsigned char* output = der.data();
    if (i2d_X509(certificate, &output) != size) {
        throw std::runtime_error(openssl_compat::error_string("encoding certificate"));
    }
    return der;
}

std::vector<unsigned char> load_certificate_der(const std::string& path) {
    BIO* file = BIO_new_file(path.c_str(), "rb");
    if (file == nullptr) {
        throw std::runtime_error(openssl_compat::error_string("opening pinned certificate"));
    }
    X509* certificate = PEM_read_bio_X509(file, nullptr, nullptr, nullptr);
    BIO_free(file);
    if (certificate == nullptr) {
        throw std::runtime_error(openssl_compat::error_string("reading pinned certificate"));
    }
    const std::vector<unsigned char> der = certificate_der(certificate);
    X509_free(certificate);
    return der;
}

void configure_context_identity(
    SSL_CTX* context,
    const std::string& cert_path,
    const std::string& key_path,
    const std::string& ca_path
) {
    if (SSL_CTX_use_certificate_chain_file(context, cert_path.c_str()) != 1) {
        throw std::runtime_error(openssl_compat::error_string("loading TLS certificate"));
    }
    if (SSL_CTX_use_PrivateKey_file(context, key_path.c_str(), SSL_FILETYPE_PEM) != 1) {
        throw std::runtime_error(openssl_compat::error_string("loading TLS private key"));
    }
    if (SSL_CTX_check_private_key(context) != 1) {
        throw std::runtime_error("TLS private key does not match certificate");
    }
    if (SSL_CTX_load_verify_locations(context, ca_path.c_str(), nullptr) != 1) {
        throw std::runtime_error(openssl_compat::error_string("loading TLS CA certificate"));
    }
    if (!openssl_compat::set_minimum_tls12(context)) {
        throw std::runtime_error(openssl_compat::error_string("setting TLS minimum version"));
    }
    if (!openssl_compat::configure_server_ecdh(context)) {
        throw std::runtime_error(openssl_compat::error_string("configuring TLS ECDH"));
    }
#ifdef SSL_OP_IGNORE_UNEXPECTED_EOF
    SSL_CTX_set_options(context, SSL_OP_IGNORE_UNEXPECTED_EOF);
#endif
}

void verify_peer(SSL* ssl, const std::vector<unsigned char>& pinned_peer) {
    if (SSL_get_verify_result(ssl) != X509_V_OK) {
        throw std::runtime_error("TLS peer certificate verification failed");
    }
    X509* certificate = openssl_compat::peer_certificate(ssl);
    if (certificate == nullptr) {
        throw std::runtime_error("TLS peer did not present a certificate");
    }
    if (!pinned_peer.empty() && certificate_der(certificate) != pinned_peer) {
        X509_free(certificate);
        throw std::runtime_error("pinned TLS peer certificate mismatch");
    }
    X509_free(certificate);
}

void run_handshake(SSL* ssl, SOCKET socket, bool server, unsigned long timeout_ms) {
    set_socket_timeout_ms(socket, timeout_ms);
    const int result = server ? SSL_accept(ssl) : SSL_connect(ssl);
    if (result != 1) {
        throw std::runtime_error(
            openssl_compat::error_string(server ? "TLS server handshake" : "TLS client handshake")
        );
    }
    set_socket_nonblocking(socket, true);
}

class OpenSslConnectionTransport : public ConnectionTransport {
public:
    OpenSslConnectionTransport(
        UniqueSocket socket,
        SSL* ssl,
        const std::vector<unsigned char>& pinned_peer,
        bool server,
        unsigned long handshake_timeout_ms
    )
        : socket_(std::move(socket)), ssl_(ssl), incoming_(), incoming_offset_(0U), outgoing_(),
          outgoing_offset_(0U), enqueued_bytes_(0ULL), written_bytes_(0ULL), stop_(false),
          eof_(false), failed_(false), error_(), pump_thread_() {
        try {
            run_handshake(ssl_, socket_.get(), server, handshake_timeout_ms);
            verify_peer(ssl_, pinned_peer);
            pump_thread_.reset(new std::thread(&OpenSslConnectionTransport::pump, this));
        } catch (...) {
            SSL_free(ssl_);
            ssl_ = nullptr;
            throw;
        }
    }

    ~OpenSslConnectionTransport() {
        shutdown();
        consume_daemon_thread(&pump_thread_);
        if (ssl_ != nullptr) {
            SSL_free(ssl_);
        }
    }

    SOCKET native_socket() const override { return socket_.get(); }

    int wait_readable(unsigned long timeout_ms) override {
        BasicLockGuard lock(mutex_);
        if (has_read_result_locked()) {
            return 1;
        }
        const std::uint64_t deadline = platform::monotonic_ms() + timeout_ms;
        unsigned long remaining = timeout_ms;
        while (condition_.timed_wait_ms(mutex_, remaining)) {
            if (has_read_result_locked()) {
                return 1;
            }
            const std::uint64_t now = platform::monotonic_ms();
            if (now >= deadline) {
                return 0;
            }
            remaining = static_cast<unsigned long>(deadline - now);
        }
        return 0;
    }

    int read(char* data, std::size_t size) override {
        if (size == 0U) {
            return 0;
        }
        BasicLockGuard lock(mutex_);
        while (incoming_offset_ == incoming_.size() && !eof_ && !failed_ && !stop_) {
            condition_.wait(mutex_);
        }
        const std::size_t available = incoming_.size() - incoming_offset_;
        if (available != 0U) {
            const std::size_t copied = std::min(available, bounded_socket_io_size(size));
            std::memcpy(data, incoming_.data() + incoming_offset_, copied);
            incoming_offset_ += copied;
            compact_incoming_locked();
            condition_.broadcast();
            return static_cast<int>(copied);
        }
        if (failed_) {
            throw std::runtime_error(error_);
        }
        return 0;
    }

    int write(const char* data, std::size_t size) override {
        if (size == 0U) {
            return 0;
        }
        const std::size_t bounded = bounded_socket_io_size(size);
        BasicLockGuard lock(mutex_);
        if (failed_) {
            throw std::runtime_error(error_);
        }
        if (stop_ || eof_) {
            return -1;
        }
        outgoing_.append(data, bounded);
        enqueued_bytes_ += static_cast<std::uint64_t>(bounded);
        const std::uint64_t target = enqueued_bytes_;
        condition_.broadcast();
        while (written_bytes_ < target && !failed_ && !stop_ && !eof_) {
            condition_.wait(mutex_);
        }
        if (failed_) {
            throw std::runtime_error(error_);
        }
        return written_bytes_ >= target ? static_cast<int>(bounded) : -1;
    }

    void set_timeout_ms(unsigned long) override {}

    void shutdown() override {
        {
            BasicLockGuard lock(mutex_);
            if (stop_) {
                return;
            }
            stop_ = true;
            condition_.broadcast();
        }
        shutdown_socket(socket_.get());
    }

private:
    bool has_read_result_locked() const {
        return incoming_offset_ != incoming_.size() || eof_ || failed_ || stop_;
    }

    void compact_incoming_locked() {
        if (incoming_offset_ != 0U
            && (incoming_offset_ == incoming_.size() || incoming_offset_ > TLS_IO_BUFFER_SIZE)) {
            incoming_.erase(0, incoming_offset_);
            incoming_offset_ = 0U;
        }
    }

    void compact_outgoing_locked() {
        if (outgoing_offset_ != 0U
            && (outgoing_offset_ == outgoing_.size() || outgoing_offset_ > TLS_IO_BUFFER_SIZE)) {
            outgoing_.erase(0, outgoing_offset_);
            outgoing_offset_ = 0U;
        }
    }

    void fail(const std::string& error) {
        BasicLockGuard lock(mutex_);
        failed_ = true;
        error_ = error;
        condition_.broadcast();
    }

    bool stopped() {
        BasicLockGuard lock(mutex_);
        return stop_;
    }

    bool pump_read() {
        {
            BasicLockGuard lock(mutex_);
            if (incoming_.size() - incoming_offset_ >= TLS_MAX_BUFFERED_PLAINTEXT) {
                return false;
            }
        }
        char buffer[TLS_IO_BUFFER_SIZE];
        const int result = SSL_read(ssl_, buffer, static_cast<int>(sizeof(buffer)));
        if (result > 0) {
            BasicLockGuard lock(mutex_);
            incoming_.append(buffer, static_cast<std::size_t>(result));
            condition_.broadcast();
            return true;
        }
        const int error = SSL_get_error(ssl_, result);
        if (error == SSL_ERROR_WANT_READ || error == SSL_ERROR_WANT_WRITE) {
            return false;
        }
        if (error == SSL_ERROR_ZERO_RETURN || (error == SSL_ERROR_SYSCALL && result == 0)) {
            BasicLockGuard lock(mutex_);
            eof_ = true;
            condition_.broadcast();
            return false;
        }
        fail(openssl_compat::error_string("TLS read"));
        return false;
    }

    bool pump_write() {
        // The socket is non-blocking (set in run_handshake), so SSL_write
        // returns immediately with WANT_WRITE when the send buffer is full.
        // Hold the mutex across the call and write directly from outgoing_,
        // avoiding the per-chunk heap allocation and copy.
        BasicLockGuard lock(mutex_);
        if (outgoing_offset_ == outgoing_.size()) {
            return false;
        }
        const std::size_t size = std::min(TLS_IO_BUFFER_SIZE, outgoing_.size() - outgoing_offset_);
        const int result =
            SSL_write(ssl_, outgoing_.data() + outgoing_offset_, static_cast<int>(size));
        if (result > 0) {
            outgoing_offset_ += static_cast<std::size_t>(result);
            written_bytes_ += static_cast<std::uint64_t>(result);
            compact_outgoing_locked();
            condition_.broadcast();
            return true;
        }
        const int error = SSL_get_error(ssl_, result);
        if (error == SSL_ERROR_WANT_READ || error == SSL_ERROR_WANT_WRITE) {
            return false;
        }
        // Inline fail() since it re-acquires mutex_.
        failed_ = true;
        error_ = openssl_compat::error_string("TLS write");
        condition_.broadcast();
        return false;
    }

    void pump() {
        for (;;) {
            if (stopped()) {
                return;
            }
            const bool write_progress = pump_write();
            const bool read_progress = pump_read();
            if (write_progress || read_progress) {
                continue;
            }
            const int read_ready = wait_socket_readable(socket_.get(), TLS_PUMP_WAIT_MS);
            if (read_ready < 0 && !stopped()) {
                BasicLockGuard lock(mutex_);
                eof_ = true;
                condition_.broadcast();
                return;
            }
            // Only poll for write readiness when there is pending data; on an
            // idle connection this avoids a syscall per TLS_PUMP_WAIT_MS tick.
            {
                BasicLockGuard lock(mutex_);
                if (outgoing_offset_ == outgoing_.size()) {
                    continue;
                }
            }
            (void)wait_socket_writable(socket_.get(), 1UL);
        }
    }

    UniqueSocket socket_;
    SSL* ssl_;
    BasicMutex mutex_;
    BasicCondVar condition_;
    std::string incoming_;
    std::size_t incoming_offset_;
    std::string outgoing_;
    std::size_t outgoing_offset_;
    std::uint64_t enqueued_bytes_;
    std::uint64_t written_bytes_;
    bool stop_;
    bool eof_;
    bool failed_;
    std::string error_;
    std::unique_ptr<std::thread> pump_thread_;
};

} // namespace

class TlsContext {
public:
    TlsContext(SSL_CTX* context, const std::vector<unsigned char>& pinned_peer)
        : context_(context), pinned_peer_(pinned_peer) {}

    ~TlsContext() { SSL_CTX_free(context_); }

    SSL_CTX* context() const { return context_; }
    const std::vector<unsigned char>& pinned_peer() const { return pinned_peer_; }

private:
    SSL_CTX* context_;
    std::vector<unsigned char> pinned_peer_;
};

std::shared_ptr<TlsContext> make_tls_context(
    const SSL_METHOD* (*method_fn)(void),
    const std::string& cert_pem,
    const std::string& key_pem,
    const std::string& ca_pem,
    int verify_mode,
    const std::string& pinned_cert_pem,
    const char* context_label
) {
    openssl_compat::initialize();
    SSL_CTX* context = SSL_CTX_new(method_fn());
    if (context == nullptr) {
        throw std::runtime_error(
            openssl_compat::error_string(std::string("creating TLS ") + context_label + " context")
        );
    }
    try {
        configure_context_identity(context, cert_pem, key_pem, ca_pem);
        SSL_CTX_set_verify(context, verify_mode, nullptr);
        const std::vector<unsigned char> pin = pinned_cert_pem.empty()
                                                   ? std::vector<unsigned char>()
                                                   : load_certificate_der(pinned_cert_pem);
        return std::shared_ptr<TlsContext>(new TlsContext(context, pin));
    } catch (...) {
        SSL_CTX_free(context);
        throw;
    }
}

std::shared_ptr<TlsContext> make_tls_server_context(const DaemonConfig& config) {
    return make_tls_context(
        openssl_compat::server_method,
        config.tls_cert_pem,
        config.tls_key_pem,
        config.tls_ca_pem,
        SSL_VERIFY_PEER | SSL_VERIFY_FAIL_IF_NO_PEER_CERT,
        config.tls_pinned_client_cert_pem,
        "server"
    );
}

std::shared_ptr<TlsContext> make_tls_client_context(const DaemonConfig& config) {
    return make_tls_context(
        openssl_compat::client_method,
        config.reverse_tls_cert_pem,
        config.reverse_tls_key_pem,
        config.reverse_tls_ca_pem,
        SSL_VERIFY_PEER,
        "",
        "client"
    );
}

std::shared_ptr<ConnectionTransport> make_tls_server_connection_transport(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context,
    unsigned long handshake_timeout_ms
) {
    SSL* ssl = SSL_new(context->context());
    if (ssl == nullptr || SSL_set_fd(ssl, static_cast<int>(socket.get())) != 1) {
        if (ssl != nullptr) {
            SSL_free(ssl);
        }
        throw std::runtime_error(openssl_compat::error_string("creating TLS server connection"));
    }
    return std::shared_ptr<ConnectionTransport>(new OpenSslConnectionTransport(
        std::move(socket),
        ssl,
        context->pinned_peer(),
        true,
        handshake_timeout_ms
    ));
}

std::shared_ptr<ConnectionTransport> make_tls_client_connection_transport(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context,
    const std::string& server_name,
    unsigned long handshake_timeout_ms
) {
    SSL* ssl = SSL_new(context->context());
    if (ssl == nullptr || SSL_set_fd(ssl, static_cast<int>(socket.get())) != 1
        || SSL_set_tlsext_host_name(ssl, server_name.c_str()) != 1
        || !openssl_compat::set_expected_host(ssl, server_name)) {
        if (ssl != nullptr) {
            SSL_free(ssl);
        }
        throw std::runtime_error(openssl_compat::error_string("creating TLS client connection"));
    }
    return std::shared_ptr<ConnectionTransport>(new OpenSslConnectionTransport(
        std::move(socket),
        ssl,
        context->pinned_peer(),
        false,
        handshake_timeout_ms
    ));
}

#else

class TlsContext {};

namespace {
std::runtime_error tls_disabled_error() {
    return std::runtime_error("TLS requires a C++ daemon built with TLS=openssl");
}
} // namespace

std::shared_ptr<TlsContext> make_tls_server_context(const DaemonConfig&) {
    throw tls_disabled_error();
}

std::shared_ptr<TlsContext> make_tls_client_context(const DaemonConfig&) {
    throw tls_disabled_error();
}

std::shared_ptr<ConnectionTransport> make_tls_server_connection_transport(
    UniqueSocket,
    const std::shared_ptr<TlsContext>&,
    unsigned long
) {
    throw tls_disabled_error();
}

std::shared_ptr<ConnectionTransport> make_tls_client_connection_transport(
    UniqueSocket,
    const std::shared_ptr<TlsContext>&,
    const std::string&,
    unsigned long
) {
    throw tls_disabled_error();
}

#endif
