#include "port_tunnel_internal.h"

void PortTunnelService::spawn_tcp_listener_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<RetainedTcpListener>& listener
) {
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
        std::shared_ptr<RetainedTcpListener> listener;
    };

    struct ThreadEntry {
        static DWORD WINAPI entry(LPVOID raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->service->tcp_accept_loop(context->session, context->listener);
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = shared_from_this();
    context->session = session;
    context->listener = listener;
    HANDLE handle = CreateThread(NULL, 0, &ThreadEntry::entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    std::thread([service, session, listener]() {
        service->tcp_accept_loop(session, listener);
    }).detach();
#endif
}

void PortTunnelService::tcp_accept_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<RetainedTcpListener>& listener
) {
    for (;;) {
        std::shared_ptr<PortTunnelConnection> connection = wait_for_attachment(session);
        if (connection.get() == NULL) {
            return;
        }

        const int ready =
            wait_socket_readable(listener->listener.get(), RETAINED_SOCKET_POLL_TIMEOUT_MS);
        if (ready == 0) {
            continue;
        }
        if (ready < 0) {
            if (retained_listener_closed(listener) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(
                    listener->stream_id,
                    "port_accept_failed",
                    socket_error_message("select")
                );
            }
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }

        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const SOCKET accepted = accept(
            listener->listener.get(),
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (accepted == INVALID_SOCKET) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                continue;
            }
            if (retained_listener_closed(listener) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(
                    listener->stream_id,
                    "port_accept_failed",
                    socket_error_message("accept")
                );
            }
            return;
        }
        UniqueSocket accepted_socket(accepted);
        if (!connection->owns_session(session)) {
            continue;
        }
        if (!connection->accept_session_tcp_stream(
                session,
                listener->stream_id,
                std::move(accepted_socket),
                printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)
            )) {
            if (session_is_unavailable(session)) {
                return;
            }
            continue;
        }
    }
}

void PortTunnelConnection::tcp_listen(const PortTunnelFrame& frame) {
    const std::string endpoint = normalize_port_forward_endpoint(frame_meta_string(frame, "endpoint"));
    std::string bound_endpoint;

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        std::shared_ptr<RetainedTcpListener> listener(
            new RetainedTcpListener(frame.stream_id, bind_port_forward_socket(endpoint, "tcp"))
        );
        bound_endpoint = socket_local_endpoint(listener->listener.get());
        {
            BasicLockGuard lock(session->mutex);
            session->tcp_listeners[frame.stream_id] = listener;
        }
        service_->spawn_tcp_listener_loop(session, listener);
    } else {
        UniqueSocket listener(bind_port_forward_socket(endpoint, "tcp"));
        const SOCKET listener_socket = listener.get();
        bound_endpoint = socket_local_endpoint(listener_socket);
        {
            BasicLockGuard lock(state_mutex_);
            tcp_listeners_[frame.stream_id].reset(listener.release());
        }
        spawn_tcp_accept_thread(shared_from_this(), frame.stream_id, listener_socket);
    }

    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::TcpListenOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
}

void PortTunnelConnection::tcp_accept_loop_transport_owned(
    uint32_t listener_stream_id,
    SOCKET listener_socket
) {
    for (;;) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const SOCKET accepted = accept(
            listener_socket,
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (accepted == INVALID_SOCKET) {
            return;
        }
        const uint32_t stream_id = next_daemon_stream_id_.fetch_add(2U);
        std::shared_ptr<TunnelTcpStream> stream(new TunnelTcpStream(accepted));
        {
            BasicLockGuard lock(state_mutex_);
            if (closed_.load()) {
                mark_tcp_stream_closed(stream);
                return;
            }
            tcp_streams_[stream_id] = stream;
        }
        PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpAccept, stream_id);
        frame.meta = Json{
            {"listener_stream_id", listener_stream_id},
            {"peer", printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)}
        }.dump();
        send_frame(frame);
        spawn_tcp_read_thread(shared_from_this(), stream_id, stream);
    }
}

void PortTunnelConnection::tcp_connect(const PortTunnelFrame& frame) {
    const std::string endpoint = ensure_nonzero_connect_endpoint(frame_meta_string(frame, "endpoint"));
    std::shared_ptr<TunnelTcpStream> stream(
        new TunnelTcpStream(connect_port_forward_socket(endpoint, "tcp"))
    );
    {
        BasicLockGuard lock(state_mutex_);
        tcp_streams_[frame.stream_id] = stream;
    }
    send_frame(make_empty_frame(PortTunnelFrameType::TcpConnectOk, frame.stream_id));
    spawn_tcp_read_thread(shared_from_this(), frame.stream_id, stream);
}

void PortTunnelConnection::tcp_read_loop(
    uint32_t stream_id,
    std::shared_ptr<TunnelTcpStream> stream
) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        const int received = recv(
            stream->socket.get(),
            reinterpret_cast<char*>(buffer.data()),
            static_cast<int>(buffer.size()),
            0
        );
        if (received == 0) {
            send_frame(make_empty_frame(PortTunnelFrameType::TcpEof, stream_id));
            return;
        }
        if (received < 0) {
            if (!tcp_stream_closed(stream)) {
                send_error(stream_id, "port_read_failed", socket_error_message("recv"));
            }
            return;
        }
        PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpData, stream_id);
        frame.data.assign(buffer.begin(), buffer.begin() + received);
        send_frame(frame);
    }
}

void PortTunnelConnection::tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data) {
    std::shared_ptr<TunnelTcpStream> stream;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
            tcp_streams_.find(stream_id);
        if (it == tcp_streams_.end()) {
            throw PortForwardError(400, "unknown_port_connection", "unknown tunnel tcp stream");
        }
        stream = it->second;
    }
    BasicLockGuard lock(stream->mutex);
    if (stream->closed) {
        throw PortForwardError(400, "port_connection_closed", "connection was closed");
    }
    send_all_bytes(
        stream->socket.get(),
        reinterpret_cast<const char*>(data.data()),
        data.size()
    );
}

void PortTunnelConnection::tcp_eof(uint32_t stream_id) {
    std::shared_ptr<TunnelTcpStream> stream;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
            tcp_streams_.find(stream_id);
        if (it == tcp_streams_.end()) {
            return;
        }
        stream = it->second;
    }
#ifdef _WIN32
    shutdown(stream->socket.get(), SD_SEND);
#else
    shutdown(stream->socket.get(), SHUT_WR);
#endif
}
