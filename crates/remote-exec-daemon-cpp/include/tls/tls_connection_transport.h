#pragma once

#include <memory>
#include <string>

#include "core/config.h"
#include "http/connection_transport.h"

class TlsContext;

std::shared_ptr<TlsContext> make_tls_server_context(const DaemonConfig& config);
std::shared_ptr<TlsContext> make_tls_client_context(const DaemonConfig& config);

std::shared_ptr<ConnectionTransport> make_tls_server_connection_transport(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context,
    unsigned long handshake_timeout_ms
);
// Retain a shared socket owner while the TLS handshake and pump use its socket.
std::shared_ptr<ConnectionTransport> make_tls_server_connection_transport(
    const std::shared_ptr<ConnectionTransport>& socket_owner,
    const std::shared_ptr<TlsContext>& context,
    unsigned long handshake_timeout_ms
);
std::shared_ptr<ConnectionTransport> make_tls_client_connection_transport(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context,
    const std::string& server_name,
    unsigned long handshake_timeout_ms
);
