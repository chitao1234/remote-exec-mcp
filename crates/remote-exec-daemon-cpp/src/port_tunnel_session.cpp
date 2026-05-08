#include "port_tunnel_internal.h"

std::shared_ptr<PortTunnelSession> PortTunnelService::create_session() {
    std::shared_ptr<PortTunnelSession> session;
    {
        BasicLockGuard lock(mutex_);
        std::ostringstream out;
        out << "sess_cpp_" << platform::monotonic_ms() << "_" << next_session_sequence_++;
        session.reset(new PortTunnelSession(out.str()));
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

    for (std::size_t i = 0; i < listeners.size(); ++i) {
        mark_retained_listener_closed(listeners[i]);
    }
    for (std::size_t i = 0; i < udp_binds.size(); ++i) {
        mark_udp_socket_closed(udp_binds[i]);
    }
}

bool PortTunnelService::schedule_session_expiry(
    const std::shared_ptr<PortTunnelSession>& session
) {
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    if (!service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    struct ExpiryContext {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
    };

    struct ExpiryThread {
        static DWORD WINAPI entry(LPVOID raw_context) {
            std::unique_ptr<ExpiryContext> context(static_cast<ExpiryContext*>(raw_context));
            PortTunnelWorkerLease lease(context->service);
            platform::sleep_ms(RESUME_TIMEOUT_MS);
            context->service->expire_session_if_needed(context->session);
            return 0;
        }
    };

    std::unique_ptr<ExpiryContext> context(new ExpiryContext());
    context->service = service;
    context->session = session;
    HANDLE handle = CreateThread(NULL, 0, &ExpiryThread::entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
        return true;
    }
    service->release_worker();
    return false;
#else
    try {
        std::thread([service, session]() {
            PortTunnelWorkerLease lease(service);
            platform::sleep_ms(RESUME_TIMEOUT_MS);
            service->expire_session_if_needed(session);
        }).detach();
    } catch (...) {
        service->release_worker();
        throw;
    }
    return true;
#endif
}

void PortTunnelService::expire_session_if_needed(
    const std::shared_ptr<PortTunnelSession>& session
) {
    std::vector<std::shared_ptr<RetainedTcpListener> > listeners;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_binds;
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

    for (std::size_t i = 0; i < listeners.size(); ++i) {
        mark_retained_listener_closed(listeners[i]);
    }
    for (std::size_t i = 0; i < udp_binds.size(); ++i) {
        mark_udp_socket_closed(udp_binds[i]);
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
