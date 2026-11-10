#pragma once

#include <memory>
#include <string>

#include "core/config.h"
#include "core/stream.h"

class TlsContext;

std::shared_ptr<TlsContext> make_tls_server_context(const DaemonConfig& config);
std::shared_ptr<TlsContext> make_tls_client_context(const DaemonConfig& config);

// Non-blocking TLS streams for the reactor. The caller must already have set
// the socket non-blocking; the handshake advances on read/write attempts.
std::shared_ptr<Stream> make_tls_server_stream(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context
);
std::shared_ptr<Stream> make_tls_client_stream(
    UniqueSocket socket,
    const std::shared_ptr<TlsContext>& context,
    const std::string& server_name
);
