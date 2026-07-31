#pragma once

#include <string>

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
#include <openssl/ssl.h>
#endif

namespace openssl_compat {

void initialize();
void cleanup();
std::string compile_version();
std::string runtime_version();

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
const SSL_METHOD* server_method();
const SSL_METHOD* client_method();
bool set_minimum_tls12(SSL_CTX* context);
bool configure_server_ecdh(SSL_CTX* context);
bool set_expected_host(SSL* ssl, const std::string& host);
X509* peer_certificate(SSL* ssl);
std::string error_string(const std::string& operation);
#endif

} // namespace openssl_compat
