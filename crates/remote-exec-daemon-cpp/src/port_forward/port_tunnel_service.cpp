#include <sstream>
#include <utility>

#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"
#include "port_tunnel_session_teardown.h"
#include "runtime/daemon_thread.h"

namespace {

std::string next_opaque_id(const char* prefix, std::uint64_t sequence) {
    std::ostringstream out;
    out << prefix << platform::monotonic_ms() << "_" << sequence;
    return out.str();
}

} // namespace

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
