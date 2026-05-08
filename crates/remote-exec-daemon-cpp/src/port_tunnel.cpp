#include "port_tunnel_internal.h"

const std::size_t READ_BUF_SIZE = 64U * 1024U;
const unsigned long RETAINED_SOCKET_POLL_TIMEOUT_MS = 100UL;
#ifdef REMOTE_EXEC_CPP_TESTING
const unsigned long RESUME_TIMEOUT_MS = 100UL;
#else
const unsigned long RESUME_TIMEOUT_MS = 10000UL;
#endif

PortTunnelService::PortTunnelService(const PortForwardLimitConfig& limits)
    : active_workers_(0UL),
      limits_(limits),
      next_session_sequence_(1ULL) {}

bool PortTunnelService::try_acquire_worker() {
    unsigned long current = active_workers_.load();
    while (current < limits_.max_worker_threads) {
        if (active_workers_.compare_exchange_weak(current, current + 1UL)) {
            return true;
        }
    }
    return false;
}

void PortTunnelService::release_worker() {
    active_workers_.fetch_sub(1UL);
}

unsigned long PortTunnelService::max_workers() const {
    return limits_.max_worker_threads;
}

const PortForwardLimitConfig& PortTunnelService::limits() const {
    return limits_;
}

PortTunnelWorkerLease::PortTunnelWorkerLease(
    const std::shared_ptr<PortTunnelService>& service
) : service_(service) {}

PortTunnelWorkerLease::~PortTunnelWorkerLease() {
    if (service_.get() != NULL) {
        service_->release_worker();
    }
}

std::string header_token_lower(const HttpRequest& request, const std::string& name) {
    return lowercase_ascii(request.header(name));
}

bool connection_header_has_upgrade(const HttpRequest& request) {
    const std::string value = header_token_lower(request, "connection");
    std::size_t offset = 0;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset)
        );
        if (token == "upgrade") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }
    return false;
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

void mark_tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream) {
    BasicLockGuard lock(stream->mutex);
    if (!stream->closed) {
        stream->closed = true;
        shutdown_socket(stream->socket.get());
        stream->socket.reset();
    }
}

void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(socket_value->mutex);
    if (!socket_value->closed) {
        socket_value->closed = true;
        shutdown_socket(socket_value->socket.get());
        socket_value->socket.reset();
    }
}

bool tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream) {
    BasicLockGuard lock(stream->mutex);
    return stream->closed;
}

bool udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(socket_value->mutex);
    return socket_value->closed;
}

bool retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(listener->mutex);
    return listener->closed;
}

bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session) {
    BasicLockGuard lock(session->mutex);
    return session->closed || session->expired;
}

int wait_socket_readable(SOCKET socket, unsigned long timeout_ms) {
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(socket, &readfds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);

#ifdef _WIN32
    return select(0, &readfds, NULL, NULL, &timeout);
#else
    return select(socket + 1, &readfds, NULL, NULL, &timeout);
#endif
}

void mark_retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(listener->mutex);
    if (!listener->closed) {
        listener->closed = true;
        shutdown_socket(listener->listener.get());
        listener->listener.reset();
    }
}

bool is_port_tunnel_upgrade_request(const HttpRequest& request) {
    return request.method == "POST" && request.path == "/v1/port/tunnel";
}

std::shared_ptr<PortTunnelService> create_port_tunnel_service(
    const PortForwardLimitConfig& limits
) {
    return std::shared_ptr<PortTunnelService>(new PortTunnelService(limits));
}
