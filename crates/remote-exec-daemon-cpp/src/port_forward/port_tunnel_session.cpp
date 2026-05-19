#include <sstream>
#include <utility>

#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"
#include "runtime/daemon_thread.h"

namespace {

std::string next_opaque_id(const char* prefix, std::uint64_t sequence) {
    std::ostringstream out;
    out << prefix << platform::monotonic_ms() << "_" << sequence;
    return out.str();
}

void close_connection_local_streams(ConnectionLocalStreams* local_streams) {
    std::vector<std::shared_ptr<TunnelTcpStream>> tcp_streams;
    std::vector<std::shared_ptr<TunnelUdpSocket>> udp_sockets;
    local_streams->drain(&tcp_streams, &udp_sockets);
    for (std::size_t i = 0; i < tcp_streams.size(); ++i) {
        tcp_streams[i]->close();
    }
    for (std::size_t i = 0; i < udp_sockets.size(); ++i) {
        udp_sockets[i]->close();
    }
}

void close_session_attachment(const std::shared_ptr<PortTunnelSessionAttachment>& attachment) {
    if (attachment.get() == nullptr) {
        return;
    }
    close_connection_local_streams(&attachment->local_streams);
}

PortTunnelRetainedResource take_retained_resource_locked(PortTunnelSession* session) {
    PortTunnelRetainedResource resource = session->retained_resource;
    session->retained_resource = PortTunnelRetainedResource();
    return resource;
}

void move_retained_resource_to_teardown(const PortTunnelRetainedResource& resource,
                                        PortTunnelSessionTeardown* teardown) {
    if (resource.kind == PortTunnelRetainedResourceKind::TcpListener) {
        teardown->retained_listener = resource.tcp_listener;
    } else if (resource.kind == PortTunnelRetainedResourceKind::UdpBind) {
        teardown->udp_bind = resource.udp_bind;
    }
}

bool session_state_terminal(PortTunnelSessionState state) {
    return state == PortTunnelSessionState::Closed || state == PortTunnelSessionState::Expired;
}

bool session_state_available(PortTunnelSessionState state) {
    return !session_state_terminal(state);
}

bool session_state_attached(PortTunnelSessionState state) {
    return state == PortTunnelSessionState::Attached;
}

const char* session_state_name(PortTunnelSessionState state) {
    switch (state) {
    case PortTunnelSessionState::New:
        return "new";
    case PortTunnelSessionState::Attached:
        return "attached";
    case PortTunnelSessionState::Detached:
        return "detached";
    case PortTunnelSessionState::Closed:
        return "closed";
    case PortTunnelSessionState::Expired:
        return "expired";
    }
    return "unknown";
}

const char* retained_resource_kind_name(PortTunnelRetainedResourceKind kind) {
    switch (kind) {
    case PortTunnelRetainedResourceKind::None:
        return "none";
    case PortTunnelRetainedResourceKind::TcpListener:
        return "tcp_listener";
    case PortTunnelRetainedResourceKind::UdpBind:
        return "udp_bind";
    }
    return "unknown";
}

PortTunnelSessionTeardown collect_terminal_session_teardown_locked(PortTunnelSession* session, bool mark_expired) {
    PortTunnelSessionTeardown state;
    const PortTunnelSessionState previous_state = session->state;
    const PortTunnelRetainedResourceKind previous_resource_kind = session->retained_resource.kind;
    const uint32_t previous_resource_stream_id = session->retained_resource.stream_id;
    state.transitioned = true;
    session->state = mark_expired ? PortTunnelSessionState::Expired : PortTunnelSessionState::Closed;
    session->resume_deadline_ms = 0ULL;
    state.attachment = session->attachment;
    session->attachment.reset();
    session->retained_session_budget.reset();
    move_retained_resource_to_teardown(take_retained_resource_locked(session), &state);
    session->state_changed.broadcast();
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session terminal")
                    .quoted_field("session_id", session->session_id)
                    .raw(std::string("from=") + session_state_name(previous_state))
                    .raw(std::string("to=") + session_state_name(session->state))
                    .field("generation", session->generation)
                    .raw(std::string("retained_resource=") + retained_resource_kind_name(previous_resource_kind))
                    .field("retained_stream_id", previous_resource_stream_id)
                    .str());
    return state;
}

void finish_terminal_session_teardown(const PortTunnelSessionTeardown& state) {
    close_session_attachment(state.attachment);
    if (state.retained_listener.get() != nullptr) {
        state.retained_listener->close();
    }
    if (state.udp_bind.get() != nullptr) {
        state.udp_bind->close();
    }
}

void close_retained_resource(const PortTunnelRetainedResource& resource) {
    if (resource.kind == PortTunnelRetainedResourceKind::TcpListener && resource.tcp_listener.get() != nullptr) {
        resource.tcp_listener->close();
    } else if (resource.kind == PortTunnelRetainedResourceKind::UdpBind && resource.udp_bind.get() != nullptr) {
        resource.udp_bind->close();
    }
}

} // namespace

std::shared_ptr<PortTunnelSessionAttachment>
PortTunnelSession::attach(const std::shared_ptr<PortTunnelConnection>& connection) {
    BasicLockGuard lock(mutex);
    if (!session_state_available(state)) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session attach rejected")
                        .quoted_field("session_id", session_id)
                        .raw(std::string("state=") + session_state_name(state))
                        .field("generation", generation)
                        .str());
        return std::shared_ptr<PortTunnelSessionAttachment>();
    }
    const PortTunnelSessionState previous_state = state;
    state = PortTunnelSessionState::Attached;
    resume_deadline_ms = 0ULL;
    std::shared_ptr<PortTunnelSessionAttachment> previous = attachment;
    attachment.reset(new PortTunnelSessionAttachment(connection));
    state_changed.broadcast();
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session attach")
                    .quoted_field("session_id", session_id)
                    .raw(std::string("from=") + session_state_name(previous_state))
                    .raw(std::string("to=") + session_state_name(state))
                    .field("generation", generation)
                    .bool_field("replaced_attachment", previous.get() != nullptr)
                    .str());
    return previous;
}

std::shared_ptr<PortTunnelSessionAttachment> PortTunnelSession::detach_until(std::uint64_t deadline_ms,
                                                                             bool* detached) {
    BasicLockGuard lock(mutex);
    if (!session_state_available(state)) {
        if (detached != nullptr) {
            *detached = false;
        }
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session detach rejected")
                        .quoted_field("session_id", session_id)
                        .raw(std::string("state=") + session_state_name(state))
                        .field("generation", generation)
                        .str());
        return std::shared_ptr<PortTunnelSessionAttachment>();
    }
    const PortTunnelSessionState previous_state = state;
    state = PortTunnelSessionState::Detached;
    resume_deadline_ms = deadline_ms;
    std::shared_ptr<PortTunnelSessionAttachment> previous = attachment;
    attachment.reset();
    state_changed.broadcast();
    if (detached != nullptr) {
        *detached = true;
    }
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session detach")
                    .quoted_field("session_id", session_id)
                    .raw(std::string("from=") + session_state_name(previous_state))
                    .raw(std::string("to=") + session_state_name(state))
                    .field("generation", generation)
                    .field("resume_deadline_ms", resume_deadline_ms)
                    .bool_field("had_attachment", previous.get() != nullptr)
                    .str());
    return previous;
}

PortTunnelSessionTeardown PortTunnelSession::close_terminal(bool mark_expired) {
    BasicLockGuard lock(mutex);
    if (session_state_terminal(state)) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session terminal ignored")
                        .quoted_field("session_id", session_id)
                        .raw(std::string("state=") + session_state_name(state))
                        .field("generation", generation)
                        .bool_field("mark_expired", mark_expired)
                        .str());
        return PortTunnelSessionTeardown();
    }
    return collect_terminal_session_teardown_locked(this, mark_expired);
}

PortTunnelSessionTeardown PortTunnelSession::expire_if_due(std::uint64_t now_ms) {
    BasicLockGuard lock(mutex);
    if (state != PortTunnelSessionState::Detached || attachment.get() != nullptr) {
        return PortTunnelSessionTeardown();
    }
    if (resume_deadline_ms == 0ULL || now_ms < resume_deadline_ms) {
        return PortTunnelSessionTeardown();
    }
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session expiry due")
                    .quoted_field("session_id", session_id)
                    .field("generation", generation)
                    .field("now_ms", now_ms)
                    .field("resume_deadline_ms", resume_deadline_ms)
                    .str());
    return collect_terminal_session_teardown_locked(this, true);
}

bool PortTunnelSession::detached_deadline(std::uint64_t* deadline_ms) {
    BasicLockGuard lock(mutex);
    const bool detached = state == PortTunnelSessionState::Detached && attachment.get() == nullptr &&
                          resume_deadline_ms != 0ULL;
    if (deadline_ms != nullptr) {
        *deadline_ms = resume_deadline_ms;
    }
    return detached;
}

PortTunnelSessionResumeResult PortTunnelSession::prepare_resume(std::uint64_t generation_value,
                                                                std::uint64_t now_ms) {
    BasicLockGuard lock(mutex);
    if (state == PortTunnelSessionState::Closed) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session resume rejected")
                        .quoted_field("session_id", session_id)
                        .raw("reason=closed")
                        .field("generation", generation)
                        .field("requested_generation", generation_value)
                        .str());
        return PortTunnelSessionResumeResult::Unknown;
    }
    if (session_state_attached(state) || attachment.get() != nullptr) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session resume rejected")
                        .quoted_field("session_id", session_id)
                        .raw("reason=already_attached")
                        .field("generation", generation)
                        .field("requested_generation", generation_value)
                        .str());
        return PortTunnelSessionResumeResult::AlreadyAttached;
    }
    if (state == PortTunnelSessionState::Expired || (resume_deadline_ms != 0ULL && now_ms >= resume_deadline_ms)) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session resume rejected")
                        .quoted_field("session_id", session_id)
                        .raw("reason=expired")
                        .field("generation", generation)
                        .field("requested_generation", generation_value)
                        .field("now_ms", now_ms)
                        .field("resume_deadline_ms", resume_deadline_ms)
                        .str());
        return PortTunnelSessionResumeResult::Expired;
    }
    generation = generation_value;
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session resume ready")
                    .quoted_field("session_id", session_id)
                    .field("generation", generation)
                    .field("resume_deadline_ms", resume_deadline_ms)
                    .str());
    return PortTunnelSessionResumeResult::Ready;
}

void PortTunnelSession::set_generation(std::uint64_t generation_value) {
    BasicLockGuard lock(mutex);
    generation = generation_value;
}

std::shared_ptr<PortTunnelSessionAttachment> PortTunnelSession::current_attachment() {
    BasicLockGuard lock(mutex);
    return attachment;
}

std::shared_ptr<PortTunnelConnection> PortTunnelSession::connection_for_attachment(
    const std::shared_ptr<PortTunnelSessionAttachment>& expected_attachment) {
    BasicLockGuard lock(mutex);
    if (!session_state_attached(state) || expected_attachment.get() == nullptr ||
        attachment.get() != expected_attachment.get()) {
        return std::shared_ptr<PortTunnelConnection>();
    }
    return expected_attachment->connection.lock();
}

bool PortTunnelSession::insert_tcp_stream_if_attached(
    const std::shared_ptr<PortTunnelSessionAttachment>& expected_attachment,
    const std::shared_ptr<TunnelTcpStream>& stream,
    std::uint32_t* stream_id) {
    BasicLockGuard lock(mutex);
    if (!session_state_attached(state) || attachment.get() != expected_attachment.get()) {
        return false;
    }
    if (next_daemon_stream_id > UINT32_MAX - 2U) {
        return false;
    }
    const std::uint32_t next_stream_id = next_daemon_stream_id;
    next_daemon_stream_id += 2U;
    expected_attachment->local_streams.insert_tcp(next_stream_id, stream);
    if (stream_id != nullptr) {
        *stream_id = next_stream_id;
    }
    return true;
}

SessionRetainedInstallResult
PortTunnelSession::install_tcp_listener(uint32_t stream_id, const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(mutex);
    if (!session_state_attached(state) || attachment.get() == nullptr) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("retain tcp listener rejected")
                        .quoted_field("session_id", session_id)
                        .raw(std::string("state=") + session_state_name(state))
                        .field("stream_id", stream_id)
                        .str());
        return SessionRetainedInstallResult::Unavailable;
    }
    if (retained_resource.kind != PortTunnelRetainedResourceKind::None) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("retain tcp listener rejected")
                        .quoted_field("session_id", session_id)
                        .raw("reason=conflict")
                        .raw(std::string("existing=") + retained_resource_kind_name(retained_resource.kind))
                        .field("stream_id", stream_id)
                        .field("existing_stream_id", retained_resource.stream_id)
                        .str());
        return SessionRetainedInstallResult::Conflict;
    }
    retained_resource.kind = PortTunnelRetainedResourceKind::TcpListener;
    retained_resource.stream_id = stream_id;
    retained_resource.tcp_listener = listener;
    retained_resource.udp_bind.reset();
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("retain tcp listener")
                    .quoted_field("session_id", session_id)
                    .field("stream_id", stream_id)
                    .field("generation", generation)
                    .str());
    return SessionRetainedInstallResult::Installed;
}

SessionRetainedInstallResult
PortTunnelSession::install_udp_bind(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(mutex);
    if (!session_state_attached(state) || attachment.get() == nullptr) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("retain udp bind rejected")
                        .quoted_field("session_id", session_id)
                        .raw(std::string("state=") + session_state_name(state))
                        .field("stream_id", stream_id)
                        .str());
        return SessionRetainedInstallResult::Unavailable;
    }
    if (retained_resource.kind != PortTunnelRetainedResourceKind::None) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("retain udp bind rejected")
                        .quoted_field("session_id", session_id)
                        .raw("reason=conflict")
                        .raw(std::string("existing=") + retained_resource_kind_name(retained_resource.kind))
                        .field("stream_id", stream_id)
                        .field("existing_stream_id", retained_resource.stream_id)
                        .str());
        return SessionRetainedInstallResult::Conflict;
    }
    retained_resource.kind = PortTunnelRetainedResourceKind::UdpBind;
    retained_resource.stream_id = stream_id;
    retained_resource.tcp_listener.reset();
    retained_resource.udp_bind = socket_value;
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("retain udp bind")
                    .quoted_field("session_id", session_id)
                    .field("stream_id", stream_id)
                    .field("generation", generation)
                    .str());
    return SessionRetainedInstallResult::Installed;
}

std::shared_ptr<TunnelUdpSocket> PortTunnelSession::udp_bind_for(uint32_t stream_id) {
    BasicLockGuard lock(mutex);
    if (retained_resource.kind != PortTunnelRetainedResourceKind::UdpBind ||
        retained_resource.stream_id != stream_id) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    return retained_resource.udp_bind;
}

PortTunnelRetainedResource PortTunnelSession::remove_retained_resource(uint32_t stream_id, bool* removed) {
    BasicLockGuard lock(mutex);
    if (retained_resource.kind == PortTunnelRetainedResourceKind::None || retained_resource.stream_id != stream_id) {
        if (removed != nullptr) {
            *removed = false;
        }
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("retained resource remove skipped")
                        .quoted_field("session_id", session_id)
                        .field("stream_id", stream_id)
                        .raw(std::string("existing=") + retained_resource_kind_name(retained_resource.kind))
                        .field("existing_stream_id", retained_resource.stream_id)
                        .str());
        return PortTunnelRetainedResource();
    }
    if (removed != nullptr) {
        *removed = true;
    }
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("retained resource remove")
                    .quoted_field("session_id", session_id)
                    .field("stream_id", stream_id)
                    .raw(std::string("kind=") + retained_resource_kind_name(retained_resource.kind))
                    .str());
    return take_retained_resource_locked(this);
}

std::shared_ptr<PortTunnelSessionAttachment> PortTunnelSession::wait_for_attachment(unsigned long wait_ms) {
    BasicLockGuard lock(mutex);
    for (;;) {
        if (session_state_terminal(state)) {
            return std::shared_ptr<PortTunnelSessionAttachment>();
        }
        if (attachment.get() != nullptr) {
            return attachment;
        }
        state_changed.timed_wait_ms(mutex, wait_ms);
    }
}

bool PortTunnelSession::is_unavailable() {
    BasicLockGuard lock(mutex);
    return session_state_terminal(state);
}

std::shared_ptr<PortTunnelSession> PortTunnelService::create_session() {
    if (!is_running()) {
        throw PortForwardError(400, "port_tunnel_shutting_down", "port tunnel service is shutting down");
    }

    PortTunnelBudgetLease retained_budget;
    if (!try_acquire_retained_session(&retained_budget)) {
        throw PortForwardError(400, "port_tunnel_limit_exceeded", "port tunnel retained session limit reached");
    }

    std::shared_ptr<PortTunnelSession> session;
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    {
        BasicLockGuard lock(mutex_);
        if (!is_running_locked()) {
            throw PortForwardError(400, "port_tunnel_shutting_down", "port tunnel service is shutting down");
        }
        const std::string session_id = next_opaque_id("ptun_", next_session_sequence_++);
        session.reset(new PortTunnelSession(session_id, service, std::move(retained_budget)));
        sessions_[session->session_id] = session;
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session create")
                        .quoted_field("session_id", session->session_id)
                        .field("active_sessions", sessions_.size())
                        .str());
    }
    return session;
}

std::shared_ptr<PortTunnelSession> PortTunnelService::find_session(const std::string& session_id) {
    BasicLockGuard lock(mutex_);
    if (!is_running_locked()) {
        return std::shared_ptr<PortTunnelSession>();
    }
    std::map<std::string, std::shared_ptr<PortTunnelSession>>::iterator it = sessions_.find(session_id);
    if (it == sessions_.end()) {
        return std::shared_ptr<PortTunnelSession>();
    }
    return it->second;
}

void PortTunnelService::attach_session(const std::shared_ptr<PortTunnelSession>& session,
                                       const std::shared_ptr<PortTunnelConnection>& connection) {
    close_session_attachment(session->attach(connection));
}

void PortTunnelService::detach_session(const std::shared_ptr<PortTunnelSession>& session) {
    bool detached = false;
    std::shared_ptr<PortTunnelSessionAttachment> attachment =
        session->detach_until(platform::monotonic_ms() + RESUME_TIMEOUT_MS, &detached);
    if (!detached) {
        return;
    }
    close_session_attachment(attachment);
    if (!schedule_session_expiry(session)) {
        log_message(LOG_DEBUG,
                    "port_tunnel",
                    LogMessageBuilder("session expiry schedule failed")
                        .quoted_field("session_id", session->session_id)
                        .raw("action=close")
                        .str());
        close_session(session);
    }
}

void PortTunnelService::close_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard store_lock(mutex_);
        sessions_.erase(session->session_id);
    }

    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session close requested").quoted_field("session_id", session->session_id).str());
    PortTunnelSessionTeardown teardown = session->close_terminal(false);
    finish_terminal_session_teardown(teardown);
}

void PortTunnelService::close_all_sessions_for_shutdown() {
    std::vector<std::shared_ptr<PortTunnelSession>> sessions;
    {
        BasicLockGuard store_lock(mutex_);
        for (std::map<std::string, std::shared_ptr<PortTunnelSession>>::const_iterator it = sessions_.begin();
             it != sessions_.end();
             ++it) {
            sessions.push_back(it->second);
        }
        sessions_.clear();
    }

    for (std::size_t i = 0; i < sessions.size(); ++i) {
        if (sessions[i]->is_unavailable()) {
            continue;
        }
        PortTunnelSessionTeardown teardown = sessions[i]->close_terminal(false);
        finish_terminal_session_teardown(teardown);
    }
}

SessionRetainedInstallResult PortTunnelService::install_session_tcp_listener(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id,
    const std::shared_ptr<RetainedTcpListener>& listener) {
    if (!is_running()) {
        return SessionRetainedInstallResult::Unavailable;
    }
    return session->install_tcp_listener(stream_id, listener);
}

SessionRetainedInstallResult
PortTunnelService::install_session_udp_bind(const std::shared_ptr<PortTunnelSession>& session,
                                            uint32_t stream_id,
                                            const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    if (!is_running()) {
        return SessionRetainedInstallResult::Unavailable;
    }
    return session->install_udp_bind(stream_id, socket_value);
}

std::shared_ptr<TunnelUdpSocket> PortTunnelService::session_udp_bind(const std::shared_ptr<PortTunnelSession>& session,
                                                                     uint32_t stream_id) {
    return session->udp_bind_for(stream_id);
}

bool PortTunnelService::close_session_retained_resource(const std::shared_ptr<PortTunnelSession>& session,
                                                        uint32_t stream_id) {
    bool removed = false;
    close_retained_resource(session->remove_retained_resource(stream_id, &removed));
    return removed;
}

bool PortTunnelService::schedule_session_expiry(const std::shared_ptr<PortTunnelSession>& session) {
    BasicLockGuard lock(expiry_mutex_);
    if (expiry_shutdown_) {
        return false;
    }
    if (!ensure_expiry_scheduler_started_locked()) {
        return false;
    }
    expiry_sessions_.push_back(std::weak_ptr<PortTunnelSession>(session));
    expiry_cond_.signal();
    log_message(LOG_DEBUG,
                "port_tunnel",
                LogMessageBuilder("session expiry scheduled")
                    .quoted_field("session_id", session->session_id)
                    .field("scheduled_sessions", expiry_sessions_.size())
                    .str());
    return true;
}

#ifdef _WIN32
unsigned __stdcall PortTunnelService::expiry_thread_entry(void* raw_context) {
    PortTunnelService* service = static_cast<PortTunnelService*>(raw_context);
    {
        BasicLockGuard lock(service->expiry_mutex_);
        service->expiry_thread_id_ = GetCurrentThreadId();
    }
    service->expiry_scheduler_loop();
    return 0;
}
#endif

bool PortTunnelService::ensure_expiry_scheduler_started_locked() {
    if (expiry_thread_started_) {
        return true;
    }
#ifdef _WIN32
    HANDLE handle = begin_win32_thread(&PortTunnelService::expiry_thread_entry, this);
    if (handle == nullptr) {
        return false;
    }
    expiry_thread_ = handle;
#else
    try {
        PortTunnelService* self = this;
        expiry_thread_.reset(new std::thread([self]() { self->expiry_scheduler_loop(); }));
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn session expiry scheduler", ex);
        expiry_thread_.reset();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn session expiry scheduler");
        expiry_thread_.reset();
        return false;
    }
#endif
    expiry_thread_started_ = true;
    log_message(LOG_DEBUG, "port_tunnel", "expiry scheduler started");
    return true;
}

void PortTunnelService::stop_expiry_scheduler() {
#ifdef _WIN32
    HANDLE thread = nullptr;
    DWORD thread_id = 0U;
#else
    std::unique_ptr<std::thread> thread;
#endif
    {
        BasicLockGuard lock(expiry_mutex_);
        expiry_shutdown_ = true;
        expiry_cond_.broadcast();
#ifdef _WIN32
        thread = expiry_thread_;
        expiry_thread_ = nullptr;
        thread_id = expiry_thread_id_;
        expiry_thread_id_ = 0U;
#else
        thread.swap(expiry_thread_);
#endif
    }
    log_message(LOG_DEBUG, "port_tunnel", "expiry scheduler stop requested");
#ifdef _WIN32
    consume_daemon_thread(&thread, thread_id);
#else
    consume_daemon_thread(&thread);
#endif
}

void PortTunnelService::expiry_scheduler_loop() {
    for (;;) {
        std::vector<std::shared_ptr<PortTunnelSession>> due_sessions;
        unsigned long wait_ms = RESUME_TIMEOUT_MS;
        {
            BasicLockGuard lock(expiry_mutex_);
            for (;;) {
                if (expiry_shutdown_) {
                    log_message(LOG_DEBUG, "port_tunnel", "expiry scheduler stop");
                    return;
                }

                const std::uint64_t now = platform::monotonic_ms();
                wait_ms = RESUME_TIMEOUT_MS;
                for (std::vector<std::weak_ptr<PortTunnelSession>>::iterator it = expiry_sessions_.begin();
                     it != expiry_sessions_.end();) {
                    std::shared_ptr<PortTunnelSession> session = it->lock();
                    if (session.get() == nullptr) {
                        it = expiry_sessions_.erase(it);
                        continue;
                    }

                    std::uint64_t deadline = 0ULL;
                    const bool detached = session->detached_deadline(&deadline);
                    if (!detached) {
                        it = expiry_sessions_.erase(it);
                        continue;
                    }
                    if (now >= deadline) {
                        due_sessions.push_back(session);
                        it = expiry_sessions_.erase(it);
                        continue;
                    }

                    const std::uint64_t remaining = deadline - now;
                    if (remaining < wait_ms) {
                        wait_ms = static_cast<unsigned long>(remaining);
                    }
                    ++it;
                }

                if (!due_sessions.empty()) {
                    break;
                }
                expiry_cond_.timed_wait_ms(expiry_mutex_, wait_ms);
            }
        }

        for (std::size_t i = 0; i < due_sessions.size(); ++i) {
            expire_session_if_needed(due_sessions[i]);
        }
    }
}

void PortTunnelService::expire_session_if_needed(const std::shared_ptr<PortTunnelSession>& session) {
    PortTunnelSessionTeardown teardown = session->expire_if_due(platform::monotonic_ms());
    if (teardown.transitioned) {
        BasicLockGuard store_lock(mutex_);
        sessions_.erase(session->session_id);
    }
    finish_terminal_session_teardown(teardown);
}

std::shared_ptr<PortTunnelSessionAttachment>
PortTunnelService::wait_for_attachment(const std::shared_ptr<PortTunnelSession>& session) {
    return session->wait_for_attachment(RETAINED_SOCKET_POLL_TIMEOUT_MS);
}
