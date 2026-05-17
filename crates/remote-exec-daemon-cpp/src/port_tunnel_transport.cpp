#include "port_tunnel.h"
#include "port_tunnel_connection.h"
#include "port_tunnel_sender.h"
#include "port_tunnel_service.h"
#include "server_contract.h"

struct TunnelOpenMetadata {
    std::string role;
    std::uint64_t generation;
    std::string protocol;
    bool has_resume_session_id;
    std::string resume_session_id;
};

struct TunnelCloseMetadata {
    std::uint64_t generation;
};

TunnelOpenMetadata parse_tunnel_open_metadata(const PortTunnelFrame& frame) {
    try {
        const Json meta = Json::parse(frame.meta);
        TunnelOpenMetadata parsed;
        parsed.role = meta.at("role").get<std::string>();
        parsed.generation = meta.at("generation").get<std::uint64_t>();
        parsed.protocol = meta.at("protocol").get<std::string>();
        parsed.has_resume_session_id = false;
        if (meta.contains("resume_session_id") && !meta.at("resume_session_id").is_null()) {
            parsed.has_resume_session_id = true;
            parsed.resume_session_id = meta.at("resume_session_id").get<std::string>();
        }
        return parsed;
    } catch (const Json::exception& ex) {
        throw PortForwardError(400, "invalid_port_tunnel", std::string("invalid tunnel open metadata: ") + ex.what());
    }
}

TunnelCloseMetadata parse_tunnel_close_metadata(const PortTunnelFrame& frame) {
    try {
        const Json meta = Json::parse(frame.meta);
        TunnelCloseMetadata parsed;
        parsed.generation = meta.at("generation").get<std::uint64_t>();
        return parsed;
    } catch (const Json::exception& ex) {
        throw PortForwardError(400, "invalid_port_tunnel", std::string("invalid tunnel close metadata: ") + ex.what());
    }
}

Json make_tunnel_ready_limits_json(const PortForwardLimitConfig& limits) {
    return Json{{"max_active_tcp_streams", limits.max_active_tcp_streams},
                {"max_udp_peers", limits.max_udp_binds},
                {"max_queued_bytes", limits.max_tunnel_queued_bytes}};
}

PortTunnelFrame make_tunnel_ready_frame(const PortForwardLimitConfig& limits,
                                        std::uint64_t generation,
                                        const std::string* session_id,
                                        const unsigned long* resume_timeout_ms) {
    PortTunnelFrame ready = make_empty_frame(PortTunnelFrameType::TunnelReady, 0U);
    Json meta = {{"generation", generation}, {"limits", make_tunnel_ready_limits_json(limits)}};
    if (session_id != nullptr) {
        meta["session_id"] = *session_id;
    }
    if (resume_timeout_ms != nullptr) {
        meta["resume_timeout_ms"] = *resume_timeout_ms;
    }
    ready.meta = meta.dump();
    return ready;
}

int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        HttpResponse response;
        write_bearer_auth_challenge(response);
        write_request_id_header(response, request);
        send_all(client, render_http_response(response));
        return response.status;
    }
    if (request.method != "POST" ||
        request.path != server_contract::route_path(server_contract::ROUTE_PORT_TUNNEL) ||
        !connection_header_has_upgrade(request) ||
        header_token_lower(request, "upgrade") != server_contract::PORT_TUNNEL_UPGRADE_TOKEN ||
        request.header(server_contract::PORT_TUNNEL_VERSION_HEADER) != server_contract::PORT_TUNNEL_VERSION_VALUE) {
        HttpResponse response;
        write_rpc_error(response, 400, "bad_request", "invalid port tunnel upgrade request");
        write_request_id_header(response, request);
        send_all(client, render_http_response(response));
        return response.status;
    }

    const std::string request_id = request_id_for_request(request);
    std::map<std::string, std::string> response_headers;
    response_headers[request_id_header_name()] = request_id;
    send_all(client, render_http_upgrade_response(server_contract::PORT_TUNNEL_UPGRADE_TOKEN, response_headers));
    if (!state.port_tunnel_service) {
        state.port_tunnel_service = create_port_tunnel_service(state.config.port_forward_limits);
    }
    set_socket_timeout_ms(client, state.config.port_forward_limits.tunnel_io_timeout_ms);
    std::shared_ptr<PortTunnelConnection> tunnel(new PortTunnelConnection(client, state.port_tunnel_service));
    tunnel->run();
    return 101;
}

PortTunnelConnection::PortTunnelConnection(SOCKET client, const std::shared_ptr<PortTunnelService>& service)
    : client_(client), service_(service), sender_(new PortTunnelSender(client, service)), generation_(0ULL),
      mode_(PortTunnelMode::Unopened), protocol_(PortTunnelProtocol::None) {}

bool PortTunnelConnection::read_exact(unsigned char* data, std::size_t size) {
    std::size_t offset = 0;
    const std::uint64_t started_at_ms = platform::monotonic_ms();
    const unsigned long timeout_ms = service_->limits().tunnel_io_timeout_ms;
    while (offset < size) {
        if (closed()) {
            mark_closed();
            return false;
        }
        const std::uint64_t elapsed_ms = platform::monotonic_ms() - started_at_ms;
        if (elapsed_ms >= timeout_ms) {
            mark_closed();
            return false;
        }
        const unsigned long remaining_ms = timeout_ms - static_cast<unsigned long>(elapsed_ms);
        const unsigned long wait_ms = std::min<unsigned long>(remaining_ms, RETAINED_SOCKET_POLL_TIMEOUT_MS);
        const int ready = wait_socket_readable(client_, wait_ms);
        if (ready < 0) {
            mark_closed();
            return false;
        }
        if (ready == 0) {
            continue;
        }
        const int received = recv_bounded(client_, reinterpret_cast<char*>(data + offset), size - offset, 0);
        if (received == 0) {
            return false;
        }
        if (received < 0) {
            return false;
        }
        offset += static_cast<std::size_t>(received);
    }
    return true;
}

bool PortTunnelConnection::read_preface() {
    std::vector<unsigned char> bytes(port_tunnel_preface_size(), 0U);
    if (!read_exact(bytes.data(), bytes.size())) {
        return false;
    }
    return std::string(reinterpret_cast<const char*>(bytes.data()), bytes.size()) ==
           std::string(port_tunnel_preface(), port_tunnel_preface_size());
}

bool PortTunnelConnection::read_frame(PortTunnelFrame* frame) {
    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    if (!read_exact(bytes.data(), bytes.size())) {
        return false;
    }
    const uint32_t meta_len = (static_cast<uint32_t>(bytes[8]) << 24) | (static_cast<uint32_t>(bytes[9]) << 16) |
                              (static_cast<uint32_t>(bytes[10]) << 8) | static_cast<uint32_t>(bytes[11]);
    const uint32_t data_len = (static_cast<uint32_t>(bytes[12]) << 24) | (static_cast<uint32_t>(bytes[13]) << 16) |
                              (static_cast<uint32_t>(bytes[14]) << 8) | static_cast<uint32_t>(bytes[15]);
    if (meta_len > PORT_TUNNEL_MAX_META_LEN || data_len > PORT_TUNNEL_MAX_DATA_LEN) {
        throw PortTunnelFrameError("port tunnel frame exceeds maximum length");
    }
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U && !read_exact(bytes.data() + PORT_TUNNEL_HEADER_LEN, meta_len + data_len)) {
        return false;
    }
    *frame = decode_port_tunnel_frame(bytes);
    return true;
}

void PortTunnelConnection::send_frame(const PortTunnelFrame& frame) {
    sender_->send_frame(frame);
}

bool PortTunnelConnection::send_data_frame_or_limit_error(const PortTunnelFrame& frame) {
    return sender_->send_data_frame_or_limit_error(*this, frame);
}

bool PortTunnelConnection::send_data_frame_or_drop_on_limit(const PortTunnelFrame& frame) {
    return sender_->send_data_frame_or_drop_on_limit(*this, frame);
}

bool PortTunnelConnection::closed() const {
    return sender_->closed();
}

void PortTunnelConnection::mark_closed() {
    sender_->mark_closed();
}

void PortTunnelConnection::run() {
    if (!read_preface()) {
        return;
    }

    PortTunnelCloseMode close_mode = PortTunnelCloseMode::RetryableDetach;
    try {
        for (;;) {
            PortTunnelFrame frame;
            if (!read_frame(&frame)) {
                break;
            }
            handle_frame(frame);
        }
    } catch (const std::exception& ex) {
        close_mode = PortTunnelCloseMode::TerminalFailure;
        send_terminal_error(0U, "invalid_port_tunnel", ex.what());
    } catch (...) {
        close_mode = PortTunnelCloseMode::TerminalFailure;
        send_terminal_error(0U, "invalid_port_tunnel", "unknown port tunnel failure");
    }
    close_current_session(close_mode);
    close_connection_local_state();
}

void PortTunnelConnection::handle_frame(const PortTunnelFrame& frame) {
    try {
        switch (frame.type) {
        case PortTunnelFrameType::TunnelOpen:
            tunnel_open(frame);
            break;
        case PortTunnelFrameType::TunnelClose:
            tunnel_close(frame);
            break;
        case PortTunnelFrameType::TunnelHeartbeat:
            tunnel_heartbeat(frame);
            break;
        case PortTunnelFrameType::TcpListen:
            tcp_listen(frame);
            break;
        case PortTunnelFrameType::TcpConnect:
            tcp_connect(frame);
            break;
        case PortTunnelFrameType::TcpData:
            tcp_data(frame.stream_id, frame.data);
            break;
        case PortTunnelFrameType::TcpEof:
            tcp_eof(frame.stream_id);
            break;
        case PortTunnelFrameType::UdpBind:
            udp_bind(frame);
            break;
        case PortTunnelFrameType::UdpDatagram:
            udp_datagram(frame);
            break;
        case PortTunnelFrameType::Close:
            close_stream(frame.stream_id);
            break;
        default:
            send_error(frame.stream_id, "invalid_port_tunnel", "unexpected frame from broker");
            break;
        }
    } catch (const PortForwardError& ex) {
        send_error(frame.stream_id, ex.code(), ex.what());
    } catch (const std::exception& ex) {
        send_error(frame.stream_id, "internal_error", ex.what());
    }
}

void PortTunnelConnection::tunnel_open(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "tunnel open must use stream_id 0");
    }
    {
        BasicLockGuard lock(state_mutex_);
        if (mode_ != PortTunnelMode::Unopened || session_.get() != nullptr) {
            throw PortForwardError(400, "port_tunnel_already_attached", "port tunnel is already open");
        }
    }

    const TunnelOpenMetadata meta = parse_tunnel_open_metadata(frame);
    const std::string role = meta.role;
    const std::uint64_t generation = meta.generation;
    const std::string protocol = meta.protocol;
    PortTunnelProtocol tunnel_protocol = PortTunnelProtocol::None;
    if (protocol == "tcp") {
        tunnel_protocol = PortTunnelProtocol::Tcp;
    } else if (protocol == "udp") {
        tunnel_protocol = PortTunnelProtocol::Udp;
    } else {
        throw PortForwardError(400, "invalid_port_tunnel", "unknown tunnel protocol");
    }
    set_generation(generation);

    if (role == "listen") {
        std::shared_ptr<PortTunnelSession> session;
        if (meta.has_resume_session_id) {
            const std::string session_id = meta.resume_session_id;
            session = service_->find_session(session_id);
            if (session.get() == nullptr) {
                throw PortForwardError(400, "unknown_port_tunnel_session", "unknown port tunnel session");
            }

            const PortTunnelSessionResumeResult resume_result =
                session->prepare_resume(generation, platform::monotonic_ms());
            if (resume_result == PortTunnelSessionResumeResult::Unknown) {
                throw PortForwardError(400, "unknown_port_tunnel_session", "unknown port tunnel session");
            }
            if (resume_result == PortTunnelSessionResumeResult::AlreadyAttached) {
                throw PortForwardError(
                    400, "port_tunnel_already_attached", "port tunnel session is already attached");
            }
            if (resume_result == PortTunnelSessionResumeResult::Expired) {
                service_->close_session(session);
                throw PortForwardError(400, "port_tunnel_resume_expired", "port tunnel resume expired");
            }
        } else {
            session = service_->create_session();
            session->set_generation(generation);
        }

        {
            BasicLockGuard lock(state_mutex_);
            session_ = session;
            mode_ = PortTunnelMode::Listen;
            protocol_ = tunnel_protocol;
        }
        service_->attach_session(session, shared_from_this());

        const PortForwardLimitConfig& limits = service_->limits();
        const unsigned long resume_timeout_ms = RESUME_TIMEOUT_MS;
        PortTunnelFrame ready = make_tunnel_ready_frame(limits, generation, &session->session_id, &resume_timeout_ms);
        send_frame(ready);
        return;
    }

    if (role == "connect") {
        {
            BasicLockGuard lock(state_mutex_);
            mode_ = PortTunnelMode::Connect;
            protocol_ = tunnel_protocol;
        }
        const PortForwardLimitConfig& limits = service_->limits();
        PortTunnelFrame ready = make_tunnel_ready_frame(limits, generation, nullptr, nullptr);
        send_frame(ready);
        return;
    }

    throw PortForwardError(400, "invalid_port_tunnel", "unknown tunnel role");
}

void PortTunnelConnection::tunnel_close(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "tunnel close must use stream_id 0");
    }
    const TunnelCloseMetadata meta = parse_tunnel_close_metadata(frame);
    ensure_generation(meta.generation);
    PortTunnelFrame closed = make_empty_frame(PortTunnelFrameType::TunnelClosed, 0U);
    closed.meta = frame.meta;
    send_frame(closed);
    close_current_session(PortTunnelCloseMode::GracefulClose);
    close_connection_local_state();
}

void PortTunnelConnection::tunnel_heartbeat(const PortTunnelFrame& frame) {
    PortTunnelFrame ack = make_empty_frame(PortTunnelFrameType::TunnelHeartbeatAck, 0U);
    ack.meta = frame.meta;
    send_frame(ack);
}
