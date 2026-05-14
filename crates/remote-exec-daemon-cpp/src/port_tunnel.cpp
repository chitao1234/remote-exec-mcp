#include <cstdlib>

#include "port_tunnel_service.h"

const std::size_t READ_BUF_SIZE = 64U * 1024U;
const std::size_t TCP_WRITE_QUEUE_LIMIT = 8U;
const unsigned long RETAINED_SOCKET_POLL_TIMEOUT_MS = 100UL;
#ifdef REMOTE_EXEC_CPP_TESTING
const unsigned long RESUME_TIMEOUT_MS = 100UL;
#else
const unsigned long RESUME_TIMEOUT_MS = 10000UL;
#endif

void log_tunnel_exception(const char* operation, const std::exception& ex) {
    log_message(LOG_WARN, "port_tunnel", std::string(operation) + " failed: " + ex.what());
}

void log_unknown_tunnel_exception(const char* operation) {
    log_message(LOG_WARN, "port_tunnel", std::string(operation) + " failed with an unknown exception");
}

PortTunnelService::PortTunnelService(const PortForwardLimitConfig& limits)
    : active_workers_(0UL), retained_sessions_(0UL), retained_listeners_(0UL), udp_binds_(0UL),
      active_tcp_streams_(0UL), limits_(limits), next_session_sequence_(1ULL), expiry_shutdown_(false),
      expiry_thread_started_(false)
#ifdef _WIN32
      ,
      expiry_thread_(NULL)
#else
      ,
      expiry_thread_()
#endif
{
}

PortTunnelService::~PortTunnelService() {
    stop_expiry_scheduler();
}

static bool try_acquire_counter(std::atomic<unsigned long>& counter, unsigned long limit) {
    unsigned long current = counter.load();
    while (current < limit) {
        if (counter.compare_exchange_weak(current, current + 1UL)) {
            return true;
        }
    }
    return false;
}

static void release_counter(std::atomic<unsigned long>& counter, const char* counter_name) {
    unsigned long current = counter.load();
    while (current > 0UL) {
        if (counter.compare_exchange_weak(current, current - 1UL)) {
            return;
        }
    }
    log_message(LOG_ERROR, "port_tunnel", std::string("attempted to release exhausted counter `") + counter_name + "`");
    std::abort();
}

bool PortTunnelService::try_acquire_worker() {
    return try_acquire_counter(active_workers_, limits_.max_worker_threads);
}

void PortTunnelService::release_worker() {
    release_counter(active_workers_, "active_workers");
}

unsigned long PortTunnelService::max_workers() const {
    return limits_.max_worker_threads;
}

const PortForwardLimitConfig& PortTunnelService::limits() const {
    return limits_;
}

bool PortTunnelService::try_acquire_retained_session() {
    return try_acquire_counter(retained_sessions_, limits_.max_retained_sessions);
}

void PortTunnelService::release_retained_session() {
    release_counter(retained_sessions_, "retained_sessions");
}

bool PortTunnelService::try_acquire_retained_listener() {
    return try_acquire_counter(retained_listeners_, limits_.max_retained_listeners);
}

void PortTunnelService::release_retained_listener() {
    release_counter(retained_listeners_, "retained_listeners");
}

bool PortTunnelService::try_acquire_udp_bind() {
    return try_acquire_counter(udp_binds_, limits_.max_udp_binds);
}

void PortTunnelService::release_udp_bind() {
    release_counter(udp_binds_, "udp_binds");
}

bool PortTunnelService::try_acquire_active_tcp_stream() {
    return try_acquire_counter(active_tcp_streams_, limits_.max_active_tcp_streams);
}

void PortTunnelService::release_active_tcp_stream() {
    release_counter(active_tcp_streams_, "active_tcp_streams");
}

PortTunnelWorkerLease::PortTunnelWorkerLease(const std::shared_ptr<PortTunnelService>& service) : service_(service) {
}

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
        const std::string token =
            trim_ascii(comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset));
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
    std::shared_ptr<PortTunnelService> service_to_release;
    BasicLockGuard lock(stream->mutex);
    if (!stream->closed) {
        stream->closed = true;
        stream->writer_closed = true;
        stream->writer_shutdown_requested = true;
        stream->write_queue.clear();
        stream->writer_cond.broadcast();
        shutdown_socket(stream->socket.get());
        stream->socket.reset();
        if (stream->active_stream_budget_acquired) {
            stream->active_stream_budget_acquired = false;
            service_to_release = stream->service.lock();
        }
    }
    if (service_to_release.get() != NULL) {
        service_to_release->release_active_tcp_stream();
    }
}

void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    std::shared_ptr<PortTunnelService> service_to_release;
    BasicLockGuard lock(socket_value->mutex);
    if (!socket_value->closed) {
        socket_value->closed = true;
        shutdown_socket(socket_value->socket.get());
        socket_value->socket.reset();
        if (socket_value->udp_bind_budget_acquired) {
            socket_value->udp_bind_budget_acquired = false;
            service_to_release = socket_value->service.lock();
        }
    }
    if (service_to_release.get() != NULL) {
        service_to_release->release_udp_bind();
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
    std::shared_ptr<PortTunnelService> service_to_release;
    BasicLockGuard lock(listener->mutex);
    if (!listener->closed) {
        listener->closed = true;
        shutdown_socket(listener->listener.get());
        listener->listener.reset();
        if (listener->retained_listener_budget_acquired) {
            listener->retained_listener_budget_acquired = false;
            service_to_release = listener->service.lock();
        }
    }
    if (service_to_release.get() != NULL) {
        service_to_release->release_retained_listener();
    }
}

bool is_port_tunnel_upgrade_request(const HttpRequest& request) {
    return request.method == "POST" && request.path == "/v1/port/tunnel";
}

std::shared_ptr<PortTunnelService> create_port_tunnel_service(const PortForwardLimitConfig& limits) {
    return std::shared_ptr<PortTunnelService>(new PortTunnelService(limits));
}
