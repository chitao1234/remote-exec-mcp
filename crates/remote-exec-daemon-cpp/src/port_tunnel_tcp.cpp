#include "port_tunnel_internal.h"

bool PortTunnelService::spawn_tcp_listener_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<RetainedTcpListener>& listener
) {
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    if (!service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
        std::shared_ptr<RetainedTcpListener> listener;
    };

    struct ThreadEntry {
        static DWORD WINAPI entry(LPVOID raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            PortTunnelWorkerLease lease(context->service);
            context->service->tcp_accept_loop(context->session, context->listener);
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = service;
    context->session = session;
    context->listener = listener;
    HANDLE handle = CreateThread(NULL, 0, &ThreadEntry::entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
        return true;
    }
    service->release_worker();
    return false;
#else
    try {
        std::thread([service, session, listener]() {
            PortTunnelWorkerLease lease(service);
            service->tcp_accept_loop(session, listener);
        }).detach();
    } catch (...) {
        service->release_worker();
        throw;
    }
    return true;
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
        if (!service_->try_acquire_retained_listener()) {
            throw PortForwardError(
                400,
                "port_tunnel_limit_exceeded",
                "port tunnel retained listener limit reached"
            );
        }
        std::shared_ptr<PortTunnelSession> session = current_session();
        std::shared_ptr<RetainedTcpListener> listener;
        try {
            UniqueSocket listener_socket(bind_port_forward_socket(endpoint, "tcp"));
            listener.reset(new RetainedTcpListener(
                frame.stream_id,
                listener_socket.release(),
                service_,
                true
            ));
        } catch (...) {
            service_->release_retained_listener();
            throw;
        }
        bound_endpoint = socket_local_endpoint(listener->listener.get());
        {
            BasicLockGuard lock(session->mutex);
            session->tcp_listeners[frame.stream_id] = listener;
        }
        if (!service_->spawn_tcp_listener_loop(session, listener)) {
            fail_worker_limit(frame.stream_id);
            return;
        }
    } else {
        UniqueSocket listener(bind_port_forward_socket(endpoint, "tcp"));
        const SOCKET listener_socket = listener.get();
        bound_endpoint = socket_local_endpoint(listener_socket);
        {
            BasicLockGuard lock(state_mutex_);
            tcp_listeners_[frame.stream_id].reset(listener.release());
        }
        if (!spawn_tcp_accept_thread(service_, shared_from_this(), frame.stream_id, listener_socket)) {
            fail_worker_limit(frame.stream_id);
            return;
        }
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
        if (!service_->try_acquire_active_tcp_stream()) {
            UniqueSocket refused_socket(accepted);
            send_error(
                stream_id,
                "port_tunnel_limit_exceeded",
                "port tunnel active tcp stream limit reached"
            );
            continue;
        }
        std::shared_ptr<TunnelTcpStream> stream(
            new TunnelTcpStream(accepted, service_, true)
        );
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
        if (!spawn_tcp_read_thread(service_, shared_from_this(), stream_id, stream)) {
            fail_worker_limit(stream_id);
            return;
        }
    }
}

void PortTunnelConnection::tcp_connect(const PortTunnelFrame& frame) {
    const std::string endpoint = ensure_nonzero_connect_endpoint(frame_meta_string(frame, "endpoint"));
    if (!service_->try_acquire_active_tcp_stream()) {
        throw PortForwardError(
            400,
            "port_tunnel_limit_exceeded",
            "port tunnel active tcp stream limit reached"
        );
    }

    std::shared_ptr<TunnelTcpStream> stream;
    try {
        stream.reset(new TunnelTcpStream(
            connect_port_forward_socket(endpoint, "tcp"),
            service_,
            true
        ));
    } catch (...) {
        service_->release_active_tcp_stream();
        throw;
    }
    {
        BasicLockGuard lock(state_mutex_);
        tcp_streams_[frame.stream_id] = stream;
    }
    send_frame(make_empty_frame(PortTunnelFrameType::TcpConnectOk, frame.stream_id));
    if (!spawn_tcp_read_thread(service_, shared_from_this(), frame.stream_id, stream)) {
        fail_worker_limit(frame.stream_id);
    }
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
            if (tcp_stream_closed(stream)) {
                return;
            }
            send_frame(make_empty_frame(PortTunnelFrameType::TcpEof, stream_id));
            return;
        }
        if (received < 0) {
            if (!tcp_stream_closed(stream)) {
                send_error(stream_id, "port_read_failed", socket_error_message("recv"));
            }
            return;
        }
        if (tcp_stream_closed(stream)) {
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
