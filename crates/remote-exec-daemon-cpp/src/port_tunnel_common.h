#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>

#ifdef _WIN32
#include <windows.h>
#include <winsock2.h>
#endif

#include "http_helpers.h"
#include "logging.h"
#include "platform.h"
#include "port_forward_endpoint.h"
#include "port_forward_error.h"
#include "port_forward_socket_ops.h"
#include "port_tunnel_frame.h"
#include "server_transport.h"
#include "text_utils.h"
#ifdef _WIN32
#include "win32_thread.h"
#endif

extern const std::size_t READ_BUFFER_SIZE;
extern const std::size_t TCP_WRITE_QUEUE_LIMIT;
extern const unsigned long RETAINED_SOCKET_POLL_TIMEOUT_MS;
extern const unsigned long RESUME_TIMEOUT_MS;

void log_tunnel_exception(const char* operation, const std::exception& ex);
void log_unknown_tunnel_exception(const char* operation);

enum class PortTunnelCloseMode {
    RetryableDetach,
    GracefulClose,
    TerminalFailure,
};

enum class PortTunnelProtocol {
    None,
    Tcp,
    Udp,
};

enum class PortTunnelMode {
    Unopened,
    Listen,
    Connect,
};

std::string header_token_lower(const HttpRequest& request, const std::string& name);
bool connection_header_has_upgrade(const HttpRequest& request);
std::string frame_meta_string(const PortTunnelFrame& frame, const std::string& key);
PortTunnelFrame make_empty_frame(PortTunnelFrameType type, uint32_t stream_id);
int wait_socket_readable(SOCKET socket, unsigned long timeout_ms);
