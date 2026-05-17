#include <sstream>
#include <utility>

#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"

namespace {

std::string next_opaque_id(const char* prefix, std::uint64_t sequence) {
    std::ostringstream out;
    out << prefix << platform::monotonic_ms() << "_" << sequence;
    return out.str();
}

void close_retained_listener_for_service(const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(listener->mutex);
    listener->close_locked();
}

void close_udp_socket_for_service(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(socket_value->mutex);
    socket_value->close_locked();
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

bool session_has_retained_resource_locked(const std::shared_ptr<PortTunnelSession>& session) {
    return session->retained_resource.kind != PortTunnelRetainedResourceKind::None;
}

void clear_retained_resource_locked(const std::shared_ptr<PortTunnelSession>& session,
                                    std::shared_ptr<RetainedTcpListener>* listener,
                                    std::shared_ptr<TunnelUdpSocket>* udp_bind) {
    if (session->retained_resource.kind == PortTunnelRetainedResourceKind::TcpListener) {
        *listener = session->retained_resource.tcp_listener;
    } else if (session->retained_resource.kind == PortTunnelRetainedResourceKind::UdpBind) {
        *udp_bind = session->retained_resource.udp_bind;
    }
    session->retained_resource = PortTunnelRetainedResource();
}

struct SessionTeardownState {
    SessionTeardownState() {}

    std::shared_ptr<PortTunnelSessionAttachment> attachment;
    std::shared_ptr<RetainedTcpListener> retained_listener;
    std::shared_ptr<TunnelUdpSocket> udp_bind;
};

SessionTeardownState collect_terminal_session_teardown_locked(const std::shared_ptr<PortTunnelSession>& session,
                                                              bool mark_expired) {
    SessionTeardownState state;
    session->closed = !mark_expired;
    session->expired = mark_expired;
    session->resume_deadline_ms = 0ULL;
    state.attachment = session->attachment;
    session->attachment.reset();
    session->retained_session_budget.reset();
    clear_retained_resource_locked(session, &state.retained_listener, &state.udp_bind);
    session->state_changed.broadcast();
    return state;
}

void finish_terminal_session_teardown(const SessionTeardownState& state) {
    close_session_attachment(state.attachment);
    if (state.retained_listener.get() != nullptr) {
        close_retained_listener_for_service(state.retained_listener);
    }
    if (state.udp_bind.get() != nullptr) {
        close_udp_socket_for_service(state.udp_bind);
    }
}

} // namespace

std::shared_ptr<PortTunnelSession> PortTunnelService::create_session() {
    PortTunnelBudgetLease retained_budget;
    if (!try_acquire_retained_session(&retained_budget)) {
        throw PortForwardError(400, "port_tunnel_limit_exceeded", "port tunnel retained session limit reached");
    }

    std::shared_ptr<PortTunnelSession> session;
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    {
        BasicLockGuard lock(mutex_);
        const std::string session_id = next_opaque_id("ptun_", next_session_sequence_++);
        session.reset(new PortTunnelSession(session_id, service, std::move(retained_budget)));
        sessions_[session->session_id] = session;
    }
    return session;
}

std::shared_ptr<PortTunnelSession> PortTunnelService::find_session(const std::string& session_id) {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<PortTunnelSession>>::iterator it = sessions_.find(session_id);
    if (it == sessions_.end()) {
        return std::shared_ptr<PortTunnelSession>();
    }
    return it->second;
}

void PortTunnelService::attach_session(const std::shared_ptr<PortTunnelSession>& session,
                                       const std::shared_ptr<PortTunnelConnection>& connection) {
    BasicLockGuard lock(session->mutex);
    session->closed = false;
    session->expired = false;
    session->resume_deadline_ms = 0ULL;
    session->attachment.reset(new PortTunnelSessionAttachment(connection));
    session->state_changed.broadcast();
}

void PortTunnelService::detach_session(const std::shared_ptr<PortTunnelSession>& session) {
    std::shared_ptr<PortTunnelSessionAttachment> attachment;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired) {
            return;
        }
        session->resume_deadline_ms = platform::monotonic_ms() + RESUME_TIMEOUT_MS;
        attachment = session->attachment;
        session->attachment.reset();
        session->state_changed.broadcast();
    }
    close_session_attachment(attachment);
    if (!schedule_session_expiry(session)) {
        close_session(session);
    }
}

void PortTunnelService::close_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard store_lock(mutex_);
        sessions_.erase(session->session_id);
    }

    SessionTeardownState teardown;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed) {
            return;
        }
        teardown = collect_terminal_session_teardown_locked(session, false);
    }
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
        SessionTeardownState teardown;
        {
            BasicLockGuard lock(sessions[i]->mutex);
            if (sessions[i]->closed || sessions[i]->expired) {
                continue;
            }
            teardown = collect_terminal_session_teardown_locked(sessions[i], false);
        }
        finish_terminal_session_teardown(teardown);
    }
}

SessionRetainedInstallResult PortTunnelService::install_session_tcp_listener(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id,
    const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(session->mutex);
    if (session->closed || session->expired || session->attachment.get() == nullptr) {
        return SessionRetainedInstallResult::Unavailable;
    }
    if (session_has_retained_resource_locked(session)) {
        return SessionRetainedInstallResult::Conflict;
    }
    session->retained_resource.kind = PortTunnelRetainedResourceKind::TcpListener;
    session->retained_resource.stream_id = stream_id;
    session->retained_resource.tcp_listener = listener;
    session->retained_resource.udp_bind.reset();
    return SessionRetainedInstallResult::Installed;
}

SessionRetainedInstallResult
PortTunnelService::install_session_udp_bind(const std::shared_ptr<PortTunnelSession>& session,
                                            uint32_t stream_id,
                                            const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(session->mutex);
    if (session->closed || session->expired || session->attachment.get() == nullptr) {
        return SessionRetainedInstallResult::Unavailable;
    }
    if (session_has_retained_resource_locked(session)) {
        return SessionRetainedInstallResult::Conflict;
    }
    session->retained_resource.kind = PortTunnelRetainedResourceKind::UdpBind;
    session->retained_resource.stream_id = stream_id;
    session->retained_resource.tcp_listener.reset();
    session->retained_resource.udp_bind = socket_value;
    return SessionRetainedInstallResult::Installed;
}

std::shared_ptr<TunnelUdpSocket> PortTunnelService::session_udp_bind(const std::shared_ptr<PortTunnelSession>& session,
                                                                     uint32_t stream_id) {
    BasicLockGuard lock(session->mutex);
    if (session->retained_resource.kind != PortTunnelRetainedResourceKind::UdpBind ||
        session->retained_resource.stream_id != stream_id) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    return session->retained_resource.udp_bind;
}

bool PortTunnelService::close_session_retained_resource(const std::shared_ptr<PortTunnelSession>& session,
                                                        uint32_t stream_id) {
    std::shared_ptr<RetainedTcpListener> retained_listener;
    std::shared_ptr<TunnelUdpSocket> udp_bind;
    {
        BasicLockGuard lock(session->mutex);
        if (session->retained_resource.kind == PortTunnelRetainedResourceKind::None ||
            session->retained_resource.stream_id != stream_id) {
            return false;
        }
        clear_retained_resource_locked(session, &retained_listener, &udp_bind);
    }

    if (retained_listener.get() != nullptr) {
        retained_listener->close();
    }
    if (udp_bind.get() != nullptr) {
        udp_bind->close();
    }
    return true;
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
    return true;
}

#ifdef _WIN32
unsigned __stdcall PortTunnelService::expiry_thread_entry(void* raw_context) {
    PortTunnelService* service = static_cast<PortTunnelService*>(raw_context);
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
        expiry_thread_.reset(new std::thread(&PortTunnelService::expiry_scheduler_loop, this));
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
    return true;
}

void PortTunnelService::stop_expiry_scheduler() {
#ifdef _WIN32
    HANDLE thread = nullptr;
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
#else
        thread.swap(expiry_thread_);
#endif
    }
#ifdef _WIN32
    if (thread != nullptr) {
        WaitForSingleObject(thread, INFINITE);
        CloseHandle(thread);
    }
#else
    if (thread.get() != nullptr) {
        thread->join();
    }
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
                    bool detached = false;
                    {
                        BasicLockGuard session_lock(session->mutex);
                        detached = !session->closed && !session->expired && session->attachment.get() == nullptr &&
                                   session->resume_deadline_ms != 0ULL;
                        deadline = session->resume_deadline_ms;
                    }
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
    SessionTeardownState teardown;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired || session->attachment.get() != nullptr) {
            return;
        }
        if (session->resume_deadline_ms == 0ULL || platform::monotonic_ms() < session->resume_deadline_ms) {
            return;
        }
        teardown = collect_terminal_session_teardown_locked(session, true);
    }
    finish_terminal_session_teardown(teardown);
}

std::shared_ptr<PortTunnelSessionAttachment>
PortTunnelService::wait_for_attachment(const std::shared_ptr<PortTunnelSession>& session) {
    BasicLockGuard lock(session->mutex);
    for (;;) {
        if (session->closed || session->expired) {
            return std::shared_ptr<PortTunnelSessionAttachment>();
        }
        if (session->attachment.get() != nullptr) {
            return session->attachment;
        }
        session->state_changed.timed_wait_ms(session->mutex, RETAINED_SOCKET_POLL_TIMEOUT_MS);
    }
}
