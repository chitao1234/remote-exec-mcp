#include "port_tunnel_common.h"

#include "json.hpp"

using Json = nlohmann::json;

const std::size_t READ_BUFFER_SIZE = 64U * 1024U;
const std::size_t TCP_WRITE_QUEUE_LIMIT = 8U;
const unsigned long RETAINED_SOCKET_POLL_TIMEOUT_MS = 100UL;
#ifdef REMOTE_EXEC_CPP_TESTING
const unsigned long RESUME_TIMEOUT_MS = 1000UL;
#else
const unsigned long RESUME_TIMEOUT_MS = 10000UL;
#endif

void log_tunnel_exception(const char* operation, const std::exception& ex) {
    log_message(LOG_WARN, "port_tunnel", std::string(operation) + " failed: " + ex.what());
}

void log_unknown_tunnel_exception(const char* operation) {
    log_message(
        LOG_WARN,
        "port_tunnel",
        std::string(operation) + " failed with an unknown exception"
    );
}

const char* port_tunnel_close_mode_name(PortTunnelCloseMode mode) {
    switch (mode) {
    case PortTunnelCloseMode::RetryableDetach:
        return "retryable_detach";
    case PortTunnelCloseMode::GracefulClose:
        return "graceful_close";
    case PortTunnelCloseMode::TerminalFailure:
        return "terminal_failure";
    }
    return "unknown";
}

const char* port_tunnel_protocol_name(PortTunnelProtocol protocol) {
    switch (protocol) {
    case PortTunnelProtocol::None:
        return "none";
    case PortTunnelProtocol::Tcp:
        return "tcp";
    case PortTunnelProtocol::Udp:
        return "udp";
    }
    return "unknown";
}

const char* port_tunnel_mode_name(PortTunnelMode mode) {
    switch (mode) {
    case PortTunnelMode::Unopened:
        return "unopened";
    case PortTunnelMode::Listen:
        return "listen";
    case PortTunnelMode::Connect:
        return "connect";
    }
    return "unknown";
}

std::string frame_meta_string(const PortTunnelFrame& frame, const std::string& key) {
    return Json::parse(frame.meta).at(key).get<std::string>();
}

PortTunnelFrame make_empty_frame(PortTunnelFrameType type, uint32_t stream_id) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    return frame;
}
