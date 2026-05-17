#include <cstdlib>

#include "port_tunnel_service.h"
#include "server_contract.h"

struct PortTunnelService::TrackedWorkerThread {
    TrackedWorkerThread() : finished(false)
#ifdef _WIN32
                           ,
                           handle(nullptr),
                           thread_id(0U)
#else
                           ,
                           thread()
#endif
    {
    }

    std::atomic<bool> finished;
#ifdef _WIN32
    HANDLE handle;
    DWORD thread_id;
#else
    std::unique_ptr<std::thread> thread;
#endif
};

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
    log_message(LOG_WARN, "port_tunnel", std::string(operation) + " failed with an unknown exception");
}

PortTunnelService::PortTunnelService(const PortForwardLimitConfig& limits)
    : active_workers_(0UL), retained_sessions_(0UL), retained_listeners_(0UL), udp_binds_(0UL),
      active_tcp_streams_(0UL), limits_(limits), next_session_sequence_(1ULL), expiry_shutdown_(false),
      expiry_thread_started_(false)
#ifdef _WIN32
      ,
      expiry_thread_(nullptr)
#else
      ,
      expiry_thread_()
#endif
{
}

PortTunnelService::~PortTunnelService() {
    stop_expiry_scheduler();
    close_all_sessions_for_shutdown();
    join_all_workers();
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

bool PortTunnelService::spawn_tracked_worker(const char* operation,
                                             bool worker_acquired,
                                             const std::function<void()>& work) {
    if (!worker_acquired && !try_acquire_worker()) {
        return false;
    }

    std::vector<std::shared_ptr<TrackedWorkerThread>> finished_workers;
    collect_finished_workers(&finished_workers);

    std::shared_ptr<TrackedWorkerThread> worker(new TrackedWorkerThread());

#ifdef _WIN32
    struct Context {
        PortTunnelService* service;
        std::shared_ptr<TrackedWorkerThread> worker;
        std::function<void()> work;
        const char* operation;
    };

    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->worker->thread_id = GetCurrentThreadId();
            PortTunnelWorkerLease lease(context->service);
            try {
                context->work();
            } catch (const std::exception& ex) {
                log_tunnel_exception(context->operation, ex);
            } catch (...) {
                log_unknown_tunnel_exception(context->operation);
            }
            context->service->mark_worker_finished(context->worker);
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = this;
    context->worker = worker;
    context->work = work;
    context->operation = operation;

    HANDLE handle = begin_win32_thread(&ThreadEntry::entry, context.get());
    if (handle == nullptr) {
        join_workers(finished_workers);
        release_worker();
        return false;
    }
    worker->handle = handle;
    context.release();
#else
    try {
        worker->thread.reset(new std::thread([this, worker, work, operation]() {
            PortTunnelWorkerLease lease(this);
            try {
                work();
            } catch (const std::exception& ex) {
                log_tunnel_exception(operation, ex);
            } catch (...) {
                log_unknown_tunnel_exception(operation);
            }
            mark_worker_finished(worker);
        }));
    } catch (const std::exception& ex) {
        join_workers(finished_workers);
        log_tunnel_exception(operation, ex);
        release_worker();
        return false;
    } catch (...) {
        join_workers(finished_workers);
        log_unknown_tunnel_exception(operation);
        release_worker();
        return false;
    }
#endif

    {
        BasicLockGuard lock(worker_threads_mutex_);
        worker_threads_.push_back(worker);
    }
    join_workers(finished_workers);
    return true;
}

void PortTunnelService::mark_worker_finished(const std::shared_ptr<TrackedWorkerThread>& worker) {
    if (worker.get() != nullptr) {
        worker->finished.store(true);
    }
}

void PortTunnelService::collect_finished_workers(std::vector<std::shared_ptr<TrackedWorkerThread>>* finished_workers) {
    BasicLockGuard lock(worker_threads_mutex_);
    for (std::vector<std::shared_ptr<TrackedWorkerThread>>::iterator it = worker_threads_.begin();
         it != worker_threads_.end();) {
        if (!(*it)->finished.load()) {
            ++it;
            continue;
        }
        finished_workers->push_back(*it);
        it = worker_threads_.erase(it);
    }
}

void PortTunnelService::join_workers(const std::vector<std::shared_ptr<TrackedWorkerThread>>& workers) {
    for (std::size_t i = 0; i < workers.size(); ++i) {
#ifdef _WIN32
        if (workers[i]->handle != nullptr) {
            if (workers[i]->thread_id == GetCurrentThreadId()) {
                CloseHandle(workers[i]->handle);
                workers[i]->handle = nullptr;
                continue;
            }
            WaitForSingleObject(workers[i]->handle, INFINITE);
            CloseHandle(workers[i]->handle);
            workers[i]->handle = nullptr;
        }
#else
        if (workers[i]->thread.get() != nullptr) {
            if (workers[i]->thread->get_id() == std::this_thread::get_id()) {
                workers[i]->thread->detach();
                workers[i]->thread.reset();
                continue;
            }
            workers[i]->thread->join();
            workers[i]->thread.reset();
        }
#endif
    }
}

void PortTunnelService::join_all_workers() {
    std::vector<std::shared_ptr<TrackedWorkerThread>> workers;
    {
        BasicLockGuard lock(worker_threads_mutex_);
        workers.swap(worker_threads_);
    }
    join_workers(workers);
}

PortTunnelWorkerLease::PortTunnelWorkerLease(PortTunnelService* service) : service_(service) {
}

PortTunnelWorkerLease::~PortTunnelWorkerLease() {
    if (service_ != nullptr) {
        service_->release_worker();
    }
}

std::string header_token_lower(const HttpRequest& request, const std::string& name) {
    return lowercase_ascii(request.header(name));
}

bool connection_header_has_upgrade(const HttpRequest& request) {
    const std::string value = header_token_lower(request, "connection");
    if (value.empty()) {
        return false;
    }
    std::size_t offset = 0;
    while (offset < value.size()) {
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
    if (service_to_release.get() != nullptr) {
        service_to_release->release_active_tcp_stream();
    }
}

bool close_udp_socket_locked(TunnelUdpSocket* socket_value) {
    if (socket_value->closed) {
        return false;
    }
    socket_value->closed = true;
    shutdown_socket(socket_value->socket.get());
    socket_value->socket.reset();
    if (socket_value->udp_bind_budget_acquired) {
        socket_value->udp_bind_budget_acquired = false;
        return true;
    }
    return false;
}

void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    std::shared_ptr<PortTunnelService> service_to_release;
    {
        BasicLockGuard lock(socket_value->mutex);
        if (close_udp_socket_locked(socket_value.get())) {
            service_to_release = socket_value->service.lock();
        }
    }
    if (service_to_release.get() != nullptr) {
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
    return select(0, &readfds, nullptr, nullptr, &timeout);
#else
    return select(socket + 1, &readfds, nullptr, nullptr, &timeout);
#endif
}

bool close_retained_listener_locked(RetainedTcpListener* listener) {
    if (listener->closed) {
        return false;
    }
    listener->closed = true;
    shutdown_socket(listener->listener.get());
    listener->listener.reset();
    if (listener->retained_listener_budget_acquired) {
        listener->retained_listener_budget_acquired = false;
        return true;
    }
    return false;
}

void mark_retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener) {
    std::shared_ptr<PortTunnelService> service_to_release;
    {
        BasicLockGuard lock(listener->mutex);
        if (close_retained_listener_locked(listener.get())) {
            service_to_release = listener->service.lock();
        }
    }
    if (service_to_release.get() != nullptr) {
        service_to_release->release_retained_listener();
    }
}

bool is_port_tunnel_upgrade_request(const HttpRequest& request) {
    return request.method == "POST" &&
           request.path == server_contract::route_path(server_contract::ROUTE_PORT_TUNNEL);
}

std::shared_ptr<PortTunnelService> create_port_tunnel_service(const PortForwardLimitConfig& limits) {
    return std::shared_ptr<PortTunnelService>(new PortTunnelService(limits));
}
