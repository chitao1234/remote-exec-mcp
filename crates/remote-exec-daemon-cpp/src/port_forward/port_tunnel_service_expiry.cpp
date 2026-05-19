#include <exception>
#include <memory>
#include <vector>

#include "port_tunnel_service.h"
#include "port_tunnel_session_teardown.h"
#include "runtime/daemon_thread.h"

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
