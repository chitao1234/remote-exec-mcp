#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"

namespace {

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
