#include "tls/tls_connection_transport.h"

#include <algorithm>
#include <climits>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>
#include <vector>

#include "platform/platform.h"
#include "port_forward/port_forward_socket_ops.h"
#include "tls/openssl_compat.h"

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
#include <openssl/pem.h>
#include <openssl/x509.h>

namespace {

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

class OpenSslStream : public Stream {
public:
    OpenSslStream(
        UniqueSocket socket,
        SSL* ssl,
        const std::vector<unsigned char>& pinned_peer,
        bool server
    )
        : socket_(std::move(socket)), ssl_(ssl), pinned_peer_(pinned_peer), server_(server),
          handshake_done_(false), failed_(false), error_(), want_(0) {
        SSL_set_mode(ssl_, SSL_MODE_ENABLE_PARTIAL_WRITE | SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER);
    }

    ~OpenSslStream() override { close(); }

    SOCKET native_fd() const override { return socket_.get(); }

    int wanted_events() const override { return want_; }

    ReadResult read(char* data, std::size_t size) override {
        const IoStatus handshake = ensure_handshake();
        if (handshake != IoStatus::Ok) {
            return ReadResult(handshake, 0U);
        }
        const int result = SSL_read(ssl_, data, static_cast<int>(bounded_socket_io_size(size)));
        if (result > 0) {
            want_ = 0;
            return ReadResult(IoStatus::Ok, static_cast<std::size_t>(result));
        }
        const int error = SSL_get_error(ssl_, result);
        if (error == SSL_ERROR_WANT_READ || error == SSL_ERROR_WANT_WRITE) {
            want_ = error == SSL_ERROR_WANT_READ ? IO_READ : IO_WRITE;
            return ReadResult(IoStatus::WouldBlock, 0U);
        }
        if (error == SSL_ERROR_ZERO_RETURN || (error == SSL_ERROR_SYSCALL && result == 0)) {
            want_ = 0;
            return ReadResult(IoStatus::Eof, 0U);
        }
        fail("TLS read");
        return ReadResult(IoStatus::Error, 0U);
    }

    WriteResult write(const char* data, std::size_t size) override {
        const IoStatus handshake = ensure_handshake();
        if (handshake != IoStatus::Ok) {
            return WriteResult(handshake, 0U);
        }
        const int result = SSL_write(ssl_, data, static_cast<int>(bounded_socket_io_size(size)));
        if (result > 0) {
            want_ = 0;
            return WriteResult(IoStatus::Ok, static_cast<std::size_t>(result));
        }
        const int error = SSL_get_error(ssl_, result);
        if (error == SSL_ERROR_WANT_READ || error == SSL_ERROR_WANT_WRITE) {
            want_ = error == SSL_ERROR_WANT_READ ? IO_READ : IO_WRITE;
            return WriteResult(IoStatus::WouldBlock, 0U);
        }
        fail("TLS write");
        return WriteResult(IoStatus::Error, 0U);
    }

    void close() override {
        if (ssl_ != nullptr) {
            (void)SSL_shutdown(ssl_);
            SSL_free(ssl_);
            ssl_ = nullptr;
        }
        socket_.reset();
    }

private:
    IoStatus ensure_handshake() {
        if (handshake_done_) {
            return IoStatus::Ok;
        }
        if (failed_) {
            return IoStatus::Error;
        }
        const int result = server_ ? SSL_accept(ssl_) : SSL_connect(ssl_);
        if (result == 1) {
            try {
                verify_peer(ssl_, pinned_peer_);
            } catch (const std::exception& ex) {
                fail(ex.what());
                return IoStatus::Error;
            }
            handshake_done_ = true;
            want_ = 0;
            return IoStatus::Ok;
        }
        const int error = SSL_get_error(ssl_, result);
        if (error == SSL_ERROR_WANT_READ || error == SSL_ERROR_WANT_WRITE) {
            want_ = error == SSL_ERROR_WANT_READ ? IO_READ : IO_WRITE;
            return IoStatus::WouldBlock;
        }
        fail(server_ ? "TLS server handshake" : "TLS client handshake");
        return IoStatus::Error;
    }

    void fail(const std::string& operation) {
        failed_ = true;
        error_ = openssl_compat::error_string(operation);
        want_ = 0;
    }

    UniqueSocket socket_;
    SSL* ssl_;
    std::vector<unsigned char> pinned_peer_;
    bool server_;
    bool handshake_done_;
    bool failed_;
    std::string error_;
    int want_;
};

std::shared_ptr<Stream> make_tls_server_stream(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context
) {
    SSL* ssl = SSL_new(context->context());
    if (ssl == nullptr || SSL_set_fd(ssl, static_cast<int>(socket.get())) != 1) {
        if (ssl != nullptr) {
            SSL_free(ssl);
        }
        throw std::runtime_error(openssl_compat::error_string("creating TLS server stream"));
    }
    return std::shared_ptr<Stream>(
        new OpenSslStream(std::move(socket), ssl, context->pinned_peer(), true)
    );
}

std::shared_ptr<Stream> make_tls_client_stream(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context,
    const std::string& server_name
) {
    SSL* ssl = SSL_new(context->context());
    if (ssl == nullptr || SSL_set_fd(ssl, static_cast<int>(socket.get())) != 1
        || SSL_set_tlsext_host_name(ssl, server_name.c_str()) != 1
        || !openssl_compat::set_expected_host(ssl, server_name)) {
        if (ssl != nullptr) {
            SSL_free(ssl);
        }
        throw std::runtime_error(openssl_compat::error_string("creating TLS client stream"));
    }
    return std::shared_ptr<Stream>(
        new OpenSslStream(std::move(socket), ssl, context->pinned_peer(), false)
    );
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

std::shared_ptr<Stream> make_tls_server_stream(UniqueSocket, const std::shared_ptr<TlsContext>&) {
    throw tls_disabled_error();
}

std::shared_ptr<Stream> make_tls_client_stream(
    UniqueSocket,
    const std::shared_ptr<TlsContext>&,
    const std::string&
) {
    throw tls_disabled_error();
}

#endif
