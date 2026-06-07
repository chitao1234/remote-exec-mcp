#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <string>

#include "core/logging.h"
#include "core/text_utils.h"
#include "platform/platform.h"
#include "port_forward/port_forward_endpoint.h"
#include "port_forward/port_forward_error.h"
#include "port_forward/port_forward_socket_ops.h"
#include "port_forward/port_tunnel_frame.h"

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

const char* port_tunnel_close_mode_name(PortTunnelCloseMode mode);
const char* port_tunnel_protocol_name(PortTunnelProtocol protocol);
const char* port_tunnel_mode_name(PortTunnelMode mode);
std::string frame_meta_string(const PortTunnelFrame& frame, const std::string& key);
PortTunnelFrame make_empty_frame(PortTunnelFrameType type, uint32_t stream_id);
