#include <climits>
#include <cstdlib>

#ifndef _WIN32
#include <cerrno>
#include <poll.h>
#endif

#include "port_tunnel_service.h"
#include "port_tunnel_thread.h"
#include "server_contract.h"

struct PortTunnelService::WorkerGroup {
    struct Thread {
        Thread() : finished(false)
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

    bool spawn(const std::shared_ptr<PortTunnelService>& service,
               const char* operation,
               PortTunnelWorkerLease worker_lease,
               const std::function<void()>& work);
    void collect_finished(std::vector<std::shared_ptr<Thread>>* finished_workers);
    void join_workers(const std::vector<std::shared_ptr<Thread>>& workers);
    void join_all();

    BasicMutex mutex;
    std::vector<std::shared_ptr<Thread>> threads;
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

PortTunnelService::PortTunnelService(const PortForwardLimitConfig& limits)
    : lifecycle_state_(LifecycleState::Running), budget_state_(new PortTunnelBudgetState()),
      worker_group_(new WorkerGroup()), limits_(limits), next_session_sequence_(1ULL), expiry_shutdown_(false),
      expiry_thread_started_(false)
#ifdef _WIN32
      ,
      expiry_thread_(nullptr),
      expiry_thread_id_(0U)
#else
      ,
      expiry_thread_()
#endif
{
}

PortTunnelService::~PortTunnelService() {
    shutdown();
}

void PortTunnelService::shutdown() {
    if (!begin_shutdown()) {
        return;
    }
    stop_expiry_scheduler();
    close_all_sessions_for_shutdown();
    join_all_workers();
    finish_shutdown();
}

bool PortTunnelService::begin_shutdown() {
    BasicLockGuard lock(mutex_);
    if (lifecycle_state_ != LifecycleState::Running) {
        log_message(LOG_DEBUG, "port_tunnel", "shutdown request ignored state=not_running");
        return false;
    }
    lifecycle_state_ = LifecycleState::Stopping;
    log_message(LOG_DEBUG, "port_tunnel", "shutdown begin state=stopping");
    return true;
}

void PortTunnelService::finish_shutdown() {
    BasicLockGuard lock(mutex_);
    lifecycle_state_ = LifecycleState::Stopped;
    log_message(LOG_DEBUG, "port_tunnel", "shutdown finish state=stopped");
}

bool PortTunnelService::is_running() {
    BasicLockGuard lock(mutex_);
    return is_running_locked();
}

bool PortTunnelService::is_running_locked() const {
    return lifecycle_state_ == LifecycleState::Running;
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
    BasicLockGuard lock(mutex_);
    if (!is_running_locked()) {
        return false;
    }
    return try_acquire_counter(budget_state_->active_workers, limits_.max_worker_threads);
}

bool PortTunnelService::try_acquire_worker(PortTunnelWorkerLease* lease) {
    if (!try_acquire_worker()) {
        return false;
    }
    if (lease != nullptr) {
        *lease = PortTunnelWorkerLease(budget_state_);
    }
    return true;
}

PortTunnelBudgetLease::PortTunnelBudgetLease() : budget_state_(), kind_(PortTunnelBudgetKind::None) {
}

PortTunnelBudgetLease::~PortTunnelBudgetLease() {
    reset();
}

PortTunnelBudgetLease PortTunnelBudgetLease::adopt(const std::shared_ptr<PortTunnelBudgetState>& budget_state,
                                                   PortTunnelBudgetKind kind) {
    PortTunnelBudgetLease lease;
    lease.budget_state_ = budget_state;
    lease.kind_ = kind;
    return lease;
}

PortTunnelBudgetLease::PortTunnelBudgetLease(PortTunnelBudgetLease&& other)
    : budget_state_(other.budget_state_), kind_(other.kind_) {
    other.budget_state_.reset();
    other.kind_ = PortTunnelBudgetKind::None;
}

PortTunnelBudgetLease& PortTunnelBudgetLease::operator=(PortTunnelBudgetLease&& other) {
    if (this != &other) {
        reset();
        budget_state_ = other.budget_state_;
        kind_ = other.kind_;
        other.budget_state_.reset();
        other.kind_ = PortTunnelBudgetKind::None;
    }
    return *this;
}

void PortTunnelBudgetLease::reset() {
    if (kind_ == PortTunnelBudgetKind::None) {
        return;
    }

    const PortTunnelBudgetKind kind = kind_;
    kind_ = PortTunnelBudgetKind::None;
    std::shared_ptr<PortTunnelBudgetState> budget_state = budget_state_;
    budget_state_.reset();
    if (budget_state.get() == nullptr) {
        return;
    }

    switch (kind) {
    case PortTunnelBudgetKind::RetainedSession:
        release_counter(budget_state->retained_sessions, "retained_sessions");
        return;
    case PortTunnelBudgetKind::RetainedListener:
        release_counter(budget_state->retained_listeners, "retained_listeners");
        return;
    case PortTunnelBudgetKind::UdpBind:
        release_counter(budget_state->udp_binds, "udp_binds");
        return;
    case PortTunnelBudgetKind::ActiveTcpStream:
        release_counter(budget_state->active_tcp_streams, "active_tcp_streams");
        return;
    case PortTunnelBudgetKind::None:
        return;
    }
}

bool PortTunnelBudgetLease::valid() const {
    return kind_ != PortTunnelBudgetKind::None;
}

unsigned long PortTunnelService::max_workers() const {
    return limits_.max_worker_threads;
}

const PortForwardLimitConfig& PortTunnelService::limits() const {
    return limits_;
}

bool PortTunnelService::try_acquire_retained_session() {
    BasicLockGuard lock(mutex_);
    if (!is_running_locked()) {
        return false;
    }
    return try_acquire_counter(budget_state_->retained_sessions, limits_.max_retained_sessions);
}

bool PortTunnelService::try_acquire_retained_session(PortTunnelBudgetLease* lease) {
    if (!try_acquire_retained_session()) {
        return false;
    }
    if (lease != nullptr) {
        *lease = PortTunnelBudgetLease::adopt(budget_state_, PortTunnelBudgetKind::RetainedSession);
    }
    return true;
}

bool PortTunnelService::try_acquire_retained_listener() {
    BasicLockGuard lock(mutex_);
    if (!is_running_locked()) {
        return false;
    }
    return try_acquire_counter(budget_state_->retained_listeners, limits_.max_retained_listeners);
}

bool PortTunnelService::try_acquire_retained_listener(PortTunnelBudgetLease* lease) {
    if (!try_acquire_retained_listener()) {
        return false;
    }
    if (lease != nullptr) {
        *lease = PortTunnelBudgetLease::adopt(budget_state_, PortTunnelBudgetKind::RetainedListener);
    }
    return true;
}

bool PortTunnelService::try_acquire_udp_bind() {
    BasicLockGuard lock(mutex_);
    if (!is_running_locked()) {
        return false;
    }
    return try_acquire_counter(budget_state_->udp_binds, limits_.max_udp_binds);
}

bool PortTunnelService::try_acquire_udp_bind(PortTunnelBudgetLease* lease) {
    if (!try_acquire_udp_bind()) {
        return false;
    }
    if (lease != nullptr) {
        *lease = PortTunnelBudgetLease::adopt(budget_state_, PortTunnelBudgetKind::UdpBind);
    }
    return true;
}

bool PortTunnelService::try_acquire_active_tcp_stream() {
    BasicLockGuard lock(mutex_);
    if (!is_running_locked()) {
        return false;
    }
    return try_acquire_counter(budget_state_->active_tcp_streams, limits_.max_active_tcp_streams);
}

bool PortTunnelService::try_acquire_active_tcp_stream(PortTunnelBudgetLease* lease) {
    if (!try_acquire_active_tcp_stream()) {
        return false;
    }
    if (lease != nullptr) {
        *lease = PortTunnelBudgetLease::adopt(budget_state_, PortTunnelBudgetKind::ActiveTcpStream);
    }
    return true;
}

bool PortTunnelService::spawn_tracked_worker(const char* operation,
                                             PortTunnelWorkerLease worker_lease,
                                             const std::function<void()>& work) {
    return worker_group_->spawn(shared_from_this(), operation, std::move(worker_lease), work);
}

bool PortTunnelService::WorkerGroup::spawn(const std::shared_ptr<PortTunnelService>& service,
                                           const char* operation,
                                           PortTunnelWorkerLease worker_lease,
                                           const std::function<void()>& work) {
    if (!worker_lease.valid() && !service->try_acquire_worker(&worker_lease)) {
        return false;
    }
    std::shared_ptr<PortTunnelWorkerLease> worker_lease_holder(new PortTunnelWorkerLease(std::move(worker_lease)));

    std::vector<std::shared_ptr<Thread>> finished_workers;
    collect_finished(&finished_workers);

    std::shared_ptr<Thread> worker(new Thread());

#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<Thread> worker;
        std::shared_ptr<PortTunnelWorkerLease> worker_lease_holder;
        std::function<void()> work;
        const char* operation;
    };

    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->worker->thread_id = GetCurrentThreadId();
            log_message(LOG_DEBUG,
                        "port_tunnel",
                        LogMessageBuilder("worker start")
                            .quoted_field("operation", context->operation)
                            .field("thread_id", context->worker->thread_id)
                            .str());
            try {
                context->work();
            } catch (const std::exception& ex) {
                log_tunnel_exception(context->operation, ex);
            } catch (...) {
                log_unknown_tunnel_exception(context->operation);
            }
            log_message(LOG_DEBUG,
                        "port_tunnel",
                        LogMessageBuilder("worker finish")
                            .quoted_field("operation", context->operation)
                            .field("thread_id", context->worker->thread_id)
                            .str());
            context->worker->finished.store(true);
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = service;
    context->worker = worker;
    context->worker_lease_holder = worker_lease_holder;
    context->work = work;
    context->operation = operation;

    HANDLE handle = begin_win32_thread(&ThreadEntry::entry, context.get());
    if (handle == nullptr) {
        join_workers(finished_workers);
        return false;
    }
    worker->handle = handle;
    context.release();
    worker_lease_holder.reset();
#else
    try {
        worker->thread.reset(new std::thread([service, worker, worker_lease_holder, work, operation]() {
            log_message(LOG_DEBUG,
                        "port_tunnel",
                        LogMessageBuilder("worker start").quoted_field("operation", operation).str());
            try {
                work();
            } catch (const std::exception& ex) {
                log_tunnel_exception(operation, ex);
            } catch (...) {
                log_unknown_tunnel_exception(operation);
            }
            log_message(LOG_DEBUG,
                        "port_tunnel",
                        LogMessageBuilder("worker finish").quoted_field("operation", operation).str());
            worker->finished.store(true);
        }));
    } catch (const std::exception& ex) {
        join_workers(finished_workers);
        log_tunnel_exception(operation, ex);
        return false;
    } catch (...) {
        join_workers(finished_workers);
        log_unknown_tunnel_exception(operation);
        return false;
    }
    worker_lease_holder.reset();
#endif

    {
        BasicLockGuard lock(mutex);
        threads.push_back(worker);
    }
    join_workers(finished_workers);
    return true;
}

void PortTunnelService::WorkerGroup::collect_finished(std::vector<std::shared_ptr<Thread>>* finished_workers) {
    BasicLockGuard lock(mutex);
    for (std::vector<std::shared_ptr<Thread>>::iterator it = threads.begin(); it != threads.end();) {
        if (!(*it)->finished.load()) {
            ++it;
            continue;
        }
        finished_workers->push_back(*it);
        it = threads.erase(it);
    }
}

void PortTunnelService::WorkerGroup::join_workers(const std::vector<std::shared_ptr<Thread>>& workers) {
    for (std::size_t i = 0; i < workers.size(); ++i) {
#ifdef _WIN32
        consume_port_tunnel_thread(&workers[i]->handle, workers[i]->thread_id);
#else
        consume_port_tunnel_thread(&workers[i]->thread);
#endif
    }
}

void PortTunnelService::join_all_workers() {
    worker_group_->join_all();
}

void PortTunnelService::WorkerGroup::join_all() {
    std::vector<std::shared_ptr<Thread>> workers;
    {
        BasicLockGuard lock(mutex);
        workers.swap(threads);
    }
    join_workers(workers);
}

PortTunnelWorkerLease::PortTunnelWorkerLease() : budget_state_() {
}

PortTunnelWorkerLease::PortTunnelWorkerLease(const std::shared_ptr<PortTunnelBudgetState>& budget_state)
    : budget_state_(budget_state) {
}

PortTunnelWorkerLease::PortTunnelWorkerLease(PortTunnelWorkerLease&& other) : budget_state_(other.budget_state_) {
    other.budget_state_.reset();
}

PortTunnelWorkerLease& PortTunnelWorkerLease::operator=(PortTunnelWorkerLease&& other) {
    if (this != &other) {
        reset();
        budget_state_ = other.budget_state_;
        other.budget_state_.reset();
    }
    return *this;
}

PortTunnelWorkerLease::~PortTunnelWorkerLease() {
    reset();
}

void PortTunnelWorkerLease::reset() {
    if (budget_state_.get() != nullptr) {
        std::shared_ptr<PortTunnelBudgetState> budget_state = budget_state_;
        budget_state_.reset();
        release_counter(budget_state->active_workers, "active_workers");
    }
}

bool PortTunnelWorkerLease::valid() const {
    return budget_state_.get() != nullptr;
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

void TunnelTcpStream::close() {
    BasicLockGuard lock(mutex);
    if (!closed) {
        closed = true;
        writer_closed = true;
        writer_shutdown_requested = true;
        write_queue.clear();
        writer_cond.broadcast();
        shutdown_socket(socket.get());
        socket.reset();
        active_stream_budget.reset();
    }
}

bool TunnelTcpStream::is_closed() {
    BasicLockGuard lock(mutex);
    return closed;
}

void TunnelUdpSocket::close() {
    BasicLockGuard lock(mutex);
    if (closed) {
        return;
    }
    closed = true;
    shutdown_socket(socket.get());
    socket.reset();
    udp_bind_budget.reset();
}

bool TunnelUdpSocket::is_closed() {
    BasicLockGuard lock(mutex);
    return closed;
}

bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session) {
    return session->is_unavailable();
}

int wait_socket_readable(SOCKET socket, unsigned long timeout_ms) {
#ifdef _WIN32
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(socket, &readfds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    return select(0, &readfds, nullptr, nullptr, &timeout);
#else
    struct pollfd descriptor;
    descriptor.fd = socket;
    descriptor.events = POLLIN;
    descriptor.revents = 0;

    const int timeout = timeout_ms > static_cast<unsigned long>(INT_MAX) ? INT_MAX : static_cast<int>(timeout_ms);
    for (;;) {
        const int ready = poll(&descriptor, 1, timeout);
        if (ready >= 0) {
            return ready;
        }
        if (errno == EINTR) {
            continue;
        }
        return -1;
    }
#endif
}

void RetainedTcpListener::close() {
    BasicLockGuard lock(mutex);
    if (closed) {
        return;
    }
    closed = true;
    shutdown_socket(listener.get());
    listener.reset();
    retained_listener_budget.reset();
}

bool RetainedTcpListener::is_closed() {
    BasicLockGuard lock(mutex);
    return closed;
}

bool is_port_tunnel_upgrade_request(const HttpRequest& request) {
    return request.method == "POST" &&
           request.path == server_contract::route_path(server_contract::ROUTE_PORT_TUNNEL);
}

std::shared_ptr<PortTunnelService> create_port_tunnel_service(const PortForwardLimitConfig& limits) {
    return std::shared_ptr<PortTunnelService>(new PortTunnelService(limits));
}
