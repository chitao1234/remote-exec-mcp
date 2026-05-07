#include "port_tunnel_internal.h"

#ifdef _WIN32
struct TcpAcceptContext {
    std::shared_ptr<PortTunnelConnection>* tunnel;
    uint32_t stream_id;
    SOCKET socket;
};

DWORD WINAPI tcp_accept_thread_entry(LPVOID raw_context) {
    std::unique_ptr<TcpAcceptContext> context(static_cast<TcpAcceptContext*>(raw_context));
    std::shared_ptr<PortTunnelConnection> tunnel(*context->tunnel);
    delete context->tunnel;
    tunnel->tcp_accept_loop_transport_owned(context->stream_id, context->socket);
    return 0;
}
#endif

void spawn_tcp_accept_thread(
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    SOCKET socket
) {
#ifdef _WIN32
    std::unique_ptr<TcpAcceptContext> context(new TcpAcceptContext());
    context->tunnel = new std::shared_ptr<PortTunnelConnection>(tunnel);
    context->stream_id = stream_id;
    context->socket = socket;
    HANDLE handle = CreateThread(NULL, 0, tcp_accept_thread_entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::thread([tunnel, stream_id, socket]() {
        tunnel->tcp_accept_loop_transport_owned(stream_id, socket);
    }).detach();
#endif
}

#ifdef _WIN32
struct TcpReadContext {
    std::shared_ptr<PortTunnelConnection>* tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelTcpStream>* stream;
};

DWORD WINAPI tcp_read_thread_entry(LPVOID raw_context) {
    std::unique_ptr<TcpReadContext> context(static_cast<TcpReadContext*>(raw_context));
    std::shared_ptr<PortTunnelConnection> tunnel(*context->tunnel);
    std::shared_ptr<TunnelTcpStream> stream(*context->stream);
    delete context->tunnel;
    delete context->stream;
    tunnel->tcp_read_loop(context->stream_id, stream);
    return 0;
}
#endif

void spawn_tcp_read_thread(
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream
) {
#ifdef _WIN32
    std::unique_ptr<TcpReadContext> context(new TcpReadContext());
    context->tunnel = new std::shared_ptr<PortTunnelConnection>(tunnel);
    context->stream_id = stream_id;
    context->stream = new std::shared_ptr<TunnelTcpStream>(stream);
    HANDLE handle = CreateThread(NULL, 0, tcp_read_thread_entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::thread([tunnel, stream_id, stream]() { tunnel->tcp_read_loop(stream_id, stream); }).detach();
#endif
}

#ifdef _WIN32
struct UdpReadContext {
    std::shared_ptr<PortTunnelConnection>* tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelUdpSocket>* socket_value;
};

DWORD WINAPI udp_read_thread_entry(LPVOID raw_context) {
    std::unique_ptr<UdpReadContext> context(static_cast<UdpReadContext*>(raw_context));
    std::shared_ptr<PortTunnelConnection> tunnel(*context->tunnel);
    std::shared_ptr<TunnelUdpSocket> socket_value(*context->socket_value);
    delete context->tunnel;
    delete context->socket_value;
    tunnel->udp_read_loop_transport_owned(context->stream_id, socket_value);
    return 0;
}
#endif

void spawn_udp_read_thread(
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
#ifdef _WIN32
    std::unique_ptr<UdpReadContext> context(new UdpReadContext());
    context->tunnel = new std::shared_ptr<PortTunnelConnection>(tunnel);
    context->stream_id = stream_id;
    context->socket_value = new std::shared_ptr<TunnelUdpSocket>(socket_value);
    HANDLE handle = CreateThread(NULL, 0, udp_read_thread_entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::thread([tunnel, stream_id, socket_value]() {
        tunnel->udp_read_loop_transport_owned(stream_id, socket_value);
    }).detach();
#endif
}

int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        HttpResponse response;
        write_bearer_auth_challenge(response);
        send_all(client, render_http_response(response));
        return response.status;
    }
    if (request.method != "POST" || request.path != "/v1/port/tunnel" ||
        !connection_header_has_upgrade(request) ||
        header_token_lower(request, "upgrade") != "remote-exec-port-tunnel" ||
        request.header("x-remote-exec-port-tunnel-version") != "2") {
        HttpResponse response;
        write_rpc_error(response, 400, "bad_request", "invalid port tunnel upgrade request");
        send_all(client, render_http_response(response));
        return response.status;
    }

    send_all(
        client,
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: remote-exec-port-tunnel\r\n"
        "\r\n"
    );
    if (!state.port_tunnel_service) {
        state.port_tunnel_service = create_port_tunnel_service();
    }
    std::shared_ptr<PortTunnelConnection> tunnel(
        new PortTunnelConnection(client, state.port_tunnel_service)
    );
    tunnel->run();
    return 101;
}

bool PortTunnelConnection::read_exact(unsigned char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int received = recv(
            client_,
            reinterpret_cast<char*>(data + offset),
            static_cast<int>(size - offset),
            0
        );
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
    const uint32_t meta_len = (static_cast<uint32_t>(bytes[8]) << 24) |
                              (static_cast<uint32_t>(bytes[9]) << 16) |
                              (static_cast<uint32_t>(bytes[10]) << 8) |
                              static_cast<uint32_t>(bytes[11]);
    const uint32_t data_len = (static_cast<uint32_t>(bytes[12]) << 24) |
                              (static_cast<uint32_t>(bytes[13]) << 16) |
                              (static_cast<uint32_t>(bytes[14]) << 8) |
                              static_cast<uint32_t>(bytes[15]);
    if (meta_len > PORT_TUNNEL_MAX_META_LEN || data_len > PORT_TUNNEL_MAX_DATA_LEN) {
        throw PortTunnelFrameError("port tunnel frame exceeds maximum length");
    }
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U &&
        !read_exact(bytes.data() + PORT_TUNNEL_HEADER_LEN, meta_len + data_len)) {
        return false;
    }
    *frame = decode_port_tunnel_frame(bytes);
    return true;
}

void PortTunnelConnection::send_frame(const PortTunnelFrame& frame) {
    const std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
    BasicLockGuard lock(writer_mutex_);
    if (closed_.load()) {
        return;
    }
    try {
        send_all_bytes(client_, reinterpret_cast<const char*>(bytes.data()), bytes.size());
    } catch (const std::exception&) {
        closed_.store(true);
        shutdown_socket(client_);
    }
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
    }
    close_current_session(close_mode);
    close_transport_owned_state();
}

void PortTunnelConnection::handle_frame(const PortTunnelFrame& frame) {
    try {
        switch (frame.type) {
        case PortTunnelFrameType::SessionOpen:
            session_open(frame);
            break;
        case PortTunnelFrameType::SessionResume:
            session_resume(frame);
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

void PortTunnelConnection::session_open(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "session open must use stream_id 0");
    }
    if (session_mode_active()) {
        throw PortForwardError(
            400,
            "port_tunnel_already_attached",
            "port tunnel session already attached"
        );
    }
    std::shared_ptr<PortTunnelSession> session = service_->create_session();
    {
        BasicLockGuard lock(state_mutex_);
        session_ = session;
    }
    service_->attach_session(session, shared_from_this());

    PortTunnelFrame ready = make_empty_frame(PortTunnelFrameType::SessionReady, 0U);
    ready.meta = Json{
        {"session_id", session->session_id},
        {"resume_timeout_ms", RESUME_TIMEOUT_MS}
    }.dump();
    send_frame(ready);
}

void PortTunnelConnection::session_resume(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "session resume must use stream_id 0");
    }
    if (session_mode_active()) {
        throw PortForwardError(
            400,
            "port_tunnel_already_attached",
            "port tunnel session already attached"
        );
    }

    const std::string session_id = frame_meta_string(frame, "session_id");
    std::shared_ptr<PortTunnelSession> session = service_->find_session(session_id);
    if (session.get() == NULL) {
        throw PortForwardError(
            400,
            "unknown_port_tunnel_session",
            "unknown port tunnel session"
        );
    }

    bool expired = false;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed) {
            throw PortForwardError(
                400,
                "unknown_port_tunnel_session",
                "unknown port tunnel session"
            );
        }
        if (session->attached) {
            throw PortForwardError(
                400,
                "port_tunnel_already_attached",
                "port tunnel session is already attached"
            );
        }
        expired = session->expired ||
                  (session->resume_deadline_ms != 0ULL &&
                   platform::monotonic_ms() >= session->resume_deadline_ms);
    }
    if (expired) {
        service_->close_session(session);
        throw PortForwardError(
            400,
            "port_tunnel_resume_expired",
            "port tunnel resume expired"
        );
    }

    {
        BasicLockGuard lock(state_mutex_);
        session_ = session;
    }
    service_->attach_session(session, shared_from_this());
    send_frame(make_empty_frame(PortTunnelFrameType::SessionResumed, 0U));
}
