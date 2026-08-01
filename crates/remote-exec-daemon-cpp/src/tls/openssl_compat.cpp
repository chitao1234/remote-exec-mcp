#include "tls/openssl_compat.h"

#include <sstream>
#include <stdexcept>

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
#include <openssl/crypto.h>
#include <openssl/ec.h>
#include <openssl/err.h>
#include <openssl/obj_mac.h>
#include <openssl/opensslv.h>
#include <openssl/x509_vfy.h>

#include "platform/basic_mutex.h"

#if OPENSSL_VERSION_NUMBER < 0x10002000L
#error OpenSSL 1.0.2 or newer is required
#endif

#ifdef OPENSSL_NO_EC
#error OpenSSL EC support is required for TLS 1.2 interoperability
#endif

namespace {

BasicMutex initialization_mutex;
bool initialized = false;

#if OPENSSL_VERSION_NUMBER < 0x10100000L
BasicMutex* legacy_locks = nullptr;
int legacy_lock_count = 0;

void legacy_locking_callback(int mode, int index, const char*, int) {
    if (index < 0 || index >= legacy_lock_count) {
        return;
    }
    if ((mode & CRYPTO_LOCK) != 0) {
        legacy_locks[index].lock();
    } else {
        legacy_locks[index].unlock();
    }
}

void initialize_legacy_locks() {
    legacy_lock_count = CRYPTO_num_locks();
    legacy_locks = new BasicMutex[static_cast<std::size_t>(legacy_lock_count)];
    CRYPTO_set_locking_callback(legacy_locking_callback);
}

void cleanup_legacy_locks() {
    CRYPTO_set_locking_callback(nullptr);
    delete[] legacy_locks;
    legacy_locks = nullptr;
    legacy_lock_count = 0;
}
#endif

} // namespace
#endif

namespace openssl_compat {

void initialize() {
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
    BasicLockGuard lock(initialization_mutex);
    if (initialized) {
        return;
    }
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    SSL_library_init();
    SSL_load_error_strings();
    OpenSSL_add_all_algorithms();
    initialize_legacy_locks();
#else
    if (OPENSSL_init_ssl(OPENSSL_INIT_LOAD_SSL_STRINGS | OPENSSL_INIT_LOAD_CRYPTO_STRINGS, nullptr)
        != 1) {
        throw std::runtime_error("OpenSSL initialization failed");
    }
#endif
    initialized = true;
#endif
}

void cleanup() {
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
    BasicLockGuard lock(initialization_mutex);
    if (!initialized) {
        return;
    }
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    cleanup_legacy_locks();
    EVP_cleanup();
    ERR_free_strings();
#endif
    initialized = false;
#endif
}

std::string compile_version() {
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
    return OPENSSL_VERSION_TEXT;
#else
    return "disabled";
#endif
}

std::string runtime_version() {
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    return SSLeay_version(SSLEAY_VERSION);
#else
    return OpenSSL_version(OPENSSL_VERSION);
#endif
#else
    return "disabled";
#endif
}

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
const SSL_METHOD* server_method() {
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    return SSLv23_server_method();
#else
    return TLS_server_method();
#endif
}

const SSL_METHOD* client_method() {
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    return SSLv23_client_method();
#else
    return TLS_client_method();
#endif
}

bool set_minimum_tls12(SSL_CTX* context) {
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    SSL_CTX_set_options(
        context,
        SSL_OP_NO_SSLv2 | SSL_OP_NO_SSLv3 | SSL_OP_NO_TLSv1 | SSL_OP_NO_TLSv1_1
    );
    return true;
#else
    return SSL_CTX_set_min_proto_version(context, TLS1_2_VERSION) == 1;
#endif
}

bool configure_server_ecdh(SSL_CTX* context) {
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    EC_KEY* key = EC_KEY_new_by_curve_name(NID_X9_62_prime256v1);
    if (key == nullptr) {
        return false;
    }
    const int result = SSL_CTX_set_tmp_ecdh(context, key);
    EC_KEY_free(key);
    return result == 1;
#else
    return true;
#endif
}

bool set_expected_host(SSL* ssl, const std::string& host) {
#if OPENSSL_VERSION_NUMBER < 0x10100000L
    X509_VERIFY_PARAM* parameters = SSL_get0_param(ssl);
    return parameters != nullptr
           && X509_VERIFY_PARAM_set1_host(parameters, host.c_str(), host.size()) == 1;
#else
    return SSL_set1_host(ssl, host.c_str()) == 1;
#endif
}

X509* peer_certificate(SSL* ssl) {
#if OPENSSL_VERSION_NUMBER >= 0x30000000L
    return SSL_get1_peer_certificate(ssl);
#else
    return SSL_get_peer_certificate(ssl);
#endif
}

std::string error_string(const std::string& operation) {
    std::ostringstream message;
    message << operation;
    bool first = true;
    for (;;) {
        const unsigned long code = ERR_get_error();
        if (code == 0UL) {
            break;
        }
        char buffer[256];
        ERR_error_string_n(code, buffer, sizeof(buffer));
        message << (first ? ": " : "; ") << buffer;
        first = false;
    }
    if (first) {
        message << " failed";
    }
    return message.str();
}
#endif

} // namespace openssl_compat
