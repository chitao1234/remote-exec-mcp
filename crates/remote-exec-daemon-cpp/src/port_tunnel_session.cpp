#include "port_tunnel_internal.h"

namespace {

std::string next_opaque_id(const char* prefix, std::uint64_t sequence) {
    std::ostringstream out;
    out << prefix << platform::monotonic_ms() << "_" << sequence;
    return out.str();
}

void close_retained_listener_for_service(
    const std::shared_ptr<RetainedTcpListener>& listener,
    PortTunnelService& service
) {
    bool release_budget = false;
    {
        BasicLockGuard lock(listener->mutex);
        if (!listener->closed) {
            listener->closed = true;
            shutdown_socket(listener->listener.get());
            listener->listener.reset();
            if (listener->retained_listener_budget_acquired) {
                listener->retained_listener_budget_acquired = false;
                release_budget = true;
            }
        }
    }
    if (release_budget) {
        service.release_retained_listener();
    }
}

void close_udp_socket_for_service(
    const std::shared_ptr<TunnelUdpSocket>& socket_value,
    PortTunnelService& service
) {
    bool release_budget = false;
    {
        BasicLockGuard lock(socket_value->mutex);
        if (!socket_value->closed) {
            socket_value->closed = true;
            shutdown_socket(socket_value->socket.get());
            socket_value->socket.reset();
            if (socket_value->udp_bind_budget_acquired) {
                socket_value->udp_bind_budget_acquired = false;
                release_budget = true;
            }
        }
    }
    if (release_budget) {
        service.release_udp_bind();
    }
}

}  // namespace

std::shared_ptr<PortTunnelSession> PortTunnelService::create_session() {
    if (!try_acquire_retained_session()) {
        throw PortForwardError(
            400,
            "port_tunnel_limit_exceeded",
            "port tunnel retained session limit reached"
        );
    }

    std::shared_ptr<PortTunnelSession> session;
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    {
        BasicLockGuard lock(mutex_);
        const std::string session_id =
            next_opaque_id("ptun_", next_session_sequence_++);
        session.reset(new PortTunnelSession(session_id, service, true));
        sessions_[session->session_id] = session;
    }
    return session;
}

std::shared_ptr<PortTunnelSession> PortTunnelService::find_session(
    const std::string& session_id
) {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<PortTunnelSession> >::iterator it =
        sessions_.find(session_id);
    if (it == sessions_.end()) {
        return std::shared_ptr<PortTunnelSession>();
    }
    return it->second;
}

void PortTunnelService::attach_session(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<PortTunnelConnection>& connection
) {
    BasicLockGuard lock(session->mutex);
    session->attached = true;
    session->closed = false;
    session->expired = false;
    session->resume_deadline_ms = 0ULL;
    session->connection = connection;
    session->state_changed.broadcast();
}

void PortTunnelService::detach_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired) {
            return;
        }
        session->attached = false;
        session->resume_deadline_ms = platform::monotonic_ms() + RESUME_TIMEOUT_MS;
        session->connection.reset();
        session->state_changed.broadcast();
    }
    if (!schedule_session_expiry(session)) {
        close_session(session);
    }
}

void PortTunnelService::close_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard store_lock(mutex_);
        sessions_.erase(session->session_id);
    }

    std::vector<std::shared_ptr<RetainedTcpListener> > listeners;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_binds;
    bool release_retained_session_budget = false;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed) {
            return;
        }
        session->closed = true;
        session->expired = false;
        session->attached = false;
        session->resume_deadline_ms = 0ULL;
        session->connection.reset();
        if (session->retained_session_budget_acquired) {
            session->retained_session_budget_acquired = false;
            release_retained_session_budget = true;
        }
        for (std::map<uint32_t, std::shared_ptr<RetainedTcpListener> >::iterator it =
                 session->tcp_listeners.begin();
             it != session->tcp_listeners.end();
             ++it) {
            listeners.push_back(it->second);
        }
        session->tcp_listeners.clear();
        for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
                 session->udp_binds.begin();
             it != session->udp_binds.end();
             ++it) {
            udp_binds.push_back(it->second);
        }
        session->udp_binds.clear();
        session->state_changed.broadcast();
    }

    if (release_retained_session_budget) {
        release_retained_session();
    }
    for (std::size_t i = 0; i < listeners.size(); ++i) {
        close_retained_listener_for_service(listeners[i], *this);
    }
    for (std::size_t i = 0; i < udp_binds.size(); ++i) {
        close_udp_socket_for_service(udp_binds[i], *this);
    }
}

bool PortTunnelService::schedule_session_expiry(
    const std::shared_ptr<PortTunnelSession>& session
) {
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
    HANDLE handle =
        begin_win32_thread(&PortTunnelService::expiry_thread_entry, this);
    if (handle == NULL) {
        return false;
    }
    expiry_thread_ = handle;
#else
    try {
        expiry_thread_ = new std::thread(&PortTunnelService::expiry_scheduler_loop, this);
    } catch (...) {
        expiry_thread_ = NULL;
        return false;
    }
#endif
    expiry_thread_started_ = true;
    return true;
}

void PortTunnelService::stop_expiry_scheduler() {
#ifdef _WIN32
    HANDLE thread = NULL;
#else
    std::thread* thread = NULL;
#endif
    {
        BasicLockGuard lock(expiry_mutex_);
        expiry_shutdown_ = true;
        expiry_cond_.broadcast();
        thread = expiry_thread_;
        expiry_thread_ = NULL;
    }
#ifdef _WIN32
    if (thread != NULL) {
        WaitForSingleObject(thread, INFINITE);
        CloseHandle(thread);
    }
#else
    if (thread != NULL) {
        thread->join();
        delete thread;
    }
#endif
}

void PortTunnelService::expiry_scheduler_loop() {
    for (;;) {
        std::vector<std::shared_ptr<PortTunnelSession> > due_sessions;
        unsigned long wait_ms = RESUME_TIMEOUT_MS;
        {
            BasicLockGuard lock(expiry_mutex_);
            for (;;) {
                if (expiry_shutdown_) {
                    return;
                }

                const std::uint64_t now = platform::monotonic_ms();
                wait_ms = RESUME_TIMEOUT_MS;
                for (std::vector<std::weak_ptr<PortTunnelSession> >::iterator it =
                         expiry_sessions_.begin();
                     it != expiry_sessions_.end();) {
                    std::shared_ptr<PortTunnelSession> session = it->lock();
                    if (session.get() == NULL) {
                        it = expiry_sessions_.erase(it);
                        continue;
                    }

                    std::uint64_t deadline = 0ULL;
                    bool detached = false;
                    {
                        BasicLockGuard session_lock(session->mutex);
                        detached = !session->closed && !session->expired && !session->attached &&
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

void PortTunnelService::expire_session_if_needed(
    const std::shared_ptr<PortTunnelSession>& session
) {
    std::vector<std::shared_ptr<RetainedTcpListener> > listeners;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_binds;
    bool release_retained_session_budget = false;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired || session->attached) {
            return;
        }
        if (session->resume_deadline_ms == 0ULL ||
            platform::monotonic_ms() < session->resume_deadline_ms) {
            return;
        }
        session->expired = true;
        session->resume_deadline_ms = 0ULL;
        session->connection.reset();
        if (session->retained_session_budget_acquired) {
            session->retained_session_budget_acquired = false;
            release_retained_session_budget = true;
        }
        for (std::map<uint32_t, std::shared_ptr<RetainedTcpListener> >::iterator it =
                 session->tcp_listeners.begin();
             it != session->tcp_listeners.end();
             ++it) {
            listeners.push_back(it->second);
        }
        session->tcp_listeners.clear();
        for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
                 session->udp_binds.begin();
             it != session->udp_binds.end();
             ++it) {
            udp_binds.push_back(it->second);
        }
        session->udp_binds.clear();
        session->state_changed.broadcast();
    }

    if (release_retained_session_budget) {
        release_retained_session();
    }
    for (std::size_t i = 0; i < listeners.size(); ++i) {
        close_retained_listener_for_service(listeners[i], *this);
    }
    for (std::size_t i = 0; i < udp_binds.size(); ++i) {
        close_udp_socket_for_service(udp_binds[i], *this);
    }
}

std::shared_ptr<PortTunnelConnection> PortTunnelService::wait_for_attachment(
    const std::shared_ptr<PortTunnelSession>& session
) {
    BasicLockGuard lock(session->mutex);
    for (;;) {
        if (session->closed || session->expired) {
            return std::shared_ptr<PortTunnelConnection>();
        }
        if (session->attached) {
            std::shared_ptr<PortTunnelConnection> connection = session->connection.lock();
            if (connection.get() != NULL) {
                return connection;
            }
        }
        session->state_changed.timed_wait_ms(session->mutex, RETAINED_SOCKET_POLL_TIMEOUT_MS);
    }
}
