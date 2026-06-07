#include <sstream>
#include <utility>

#include "platform/deadline.h"
#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"
#include "port_tunnel_session_teardown.h"

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
        log_message(
            LOG_DEBUG,
            "port_tunnel",
            LogMessageBuilder("session create")
                .quoted_field("session_id", session->session_id)
                .field("active_sessions", sessions_.size())
                .str()
        );
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

bool PortTunnelService::attach_new_session(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<PortTunnelConnection>& connection,
    std::uint64_t generation
) {
    if (!is_running()) {
        return false;
    }
    return session->attach_new(connection, generation);
}

PortTunnelSessionResumeResult PortTunnelService::attach_resumed_session(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<PortTunnelConnection>& connection,
    std::uint64_t generation,
    std::uint64_t now_ms
) {
    if (!is_running()) {
        return PortTunnelSessionResumeResult::Unknown;
    }
    return session->attach_resumed(connection, generation, now_ms);
}

void PortTunnelService::detach_session(const std::shared_ptr<PortTunnelSession>& session) {
    bool detached = false;
    std::shared_ptr<PortTunnelSessionAttachment> attachment =
        session->detach_until(platform::monotonic_deadline_after_ms(RESUME_TIMEOUT_MS), &detached);
    if (!detached) {
        return;
    }
    close_session_attachment(attachment);
    if (!schedule_session_expiry(session)) {
        log_message(
            LOG_DEBUG,
            "port_tunnel",
            LogMessageBuilder("session expiry schedule failed")
                .quoted_field("session_id", session->session_id)
                .raw("action=close")
                .str()
        );
        close_session(session);
    }
}

void PortTunnelService::close_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard store_lock(mutex_);
        sessions_.erase(session->session_id);
    }

    log_message(
        LOG_DEBUG,
        "port_tunnel",
        LogMessageBuilder("session close requested").quoted_field("session_id", session->session_id).str()
    );
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
    const std::shared_ptr<RetainedTcpListener>& listener
) {
    if (!is_running()) {
        return SessionRetainedInstallResult::Unavailable;
    }
    return session->install_tcp_listener(stream_id, listener);
}

SessionRetainedInstallResult PortTunnelService::install_session_udp_bind(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
    if (!is_running()) {
        return SessionRetainedInstallResult::Unavailable;
    }
    return session->install_udp_bind(stream_id, socket_value);
}

std::shared_ptr<TunnelUdpSocket>
PortTunnelService::session_udp_bind(const std::shared_ptr<PortTunnelSession>& session, uint32_t stream_id) {
    return session->udp_bind_for(stream_id);
}

bool PortTunnelService::close_session_retained_resource(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id
) {
    bool removed = false;
    close_retained_resource(session->remove_retained_resource(stream_id, &removed));
    return removed;
}

std::shared_ptr<PortTunnelSessionAttachment>
PortTunnelService::wait_for_attachment(const std::shared_ptr<PortTunnelSession>& session) {
    return session->wait_for_attachment(RETAINED_SOCKET_POLL_TIMEOUT_MS);
}
