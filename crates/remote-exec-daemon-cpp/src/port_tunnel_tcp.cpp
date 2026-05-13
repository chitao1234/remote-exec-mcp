#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"

bool PortTunnelService::spawn_tcp_listener_loop(const std::shared_ptr<PortTunnelSession>& session,
                                                const std::shared_ptr<RetainedTcpListener>& listener,
                                                bool worker_acquired) {
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    if (!worker_acquired && !service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
        std::shared_ptr<RetainedTcpListener> listener;
    };

    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
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
    HANDLE handle = begin_win32_thread(&ThreadEntry::entry, context.get());
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
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn tcp listener thread", ex);
        service->release_worker();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn tcp listener thread");
        service->release_worker();
        return false;
    }
    return true;
#endif
}

void PortTunnelService::tcp_accept_loop(const std::shared_ptr<PortTunnelSession>& session,
                                        const std::shared_ptr<RetainedTcpListener>& listener) {
    for (;;) {
        std::shared_ptr<PortTunnelConnection> connection = wait_for_attachment(session);
        if (connection.get() == NULL) {
            return;
        }

        const int ready = wait_socket_readable(listener->listener.get(), RETAINED_SOCKET_POLL_TIMEOUT_MS);
        if (ready == 0) {
            continue;
        }
        if (ready < 0) {
            if (retained_listener_closed(listener) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(listener->stream_id, "port_accept_failed", socket_error_message("select"));
            }
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }

        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const SOCKET accepted = accept(listener->listener.get(), reinterpret_cast<sockaddr*>(&peer_address), &peer_len);
        if (accepted == INVALID_SOCKET) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                continue;
            }
            if (retained_listener_closed(listener) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(listener->stream_id, "port_accept_failed", socket_error_message("accept"));
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
                printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len))) {
            if (session_is_unavailable(session)) {
                return;
            }
            continue;
        }
    }
}

void PortTunnelConnection::tcp_listen(const PortTunnelFrame& frame) {
    require_mode(PortTunnelMode::Listen, PortTunnelProtocol::Tcp, "tcp listen requires an open tcp listen tunnel");

    const std::string endpoint = normalize_port_forward_endpoint(frame_meta_string(frame, "endpoint"));
    std::string bound_endpoint;

    if (!service_->try_acquire_retained_listener()) {
        throw PortForwardError(400, "port_tunnel_limit_exceeded", "port tunnel retained listener limit reached");
    }
    std::shared_ptr<PortTunnelSession> session = current_session();
    std::shared_ptr<RetainedTcpListener> listener;
    try {
        UniqueSocket listener_socket(bind_port_forward_socket(endpoint, "tcp"));
        listener.reset(new RetainedTcpListener(frame.stream_id, listener_socket.release(), service_, true));
    } catch (const std::exception& ex) {
        log_tunnel_exception("create tcp listener", ex);
        service_->release_retained_listener();
        throw;
    } catch (...) {
        log_unknown_tunnel_exception("create tcp listener");
        service_->release_retained_listener();
        throw;
    }
    bound_endpoint = socket_local_endpoint(listener->listener.get());
    if (!service_->try_acquire_worker()) {
        mark_retained_listener_closed(listener);
        send_worker_limit(frame.stream_id);
        return;
    }
    {
        BasicLockGuard lock(session->mutex);
        session->tcp_listeners[frame.stream_id] = listener;
    }
    if (!service_->spawn_tcp_listener_loop(session, listener, true)) {
        {
            BasicLockGuard lock(session->mutex);
            session->tcp_listeners.erase(frame.stream_id);
        }
        mark_retained_listener_closed(listener);
        send_worker_limit(frame.stream_id);
        return;
    }

    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::TcpListenOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
}

void PortTunnelConnection::tcp_connect(const PortTunnelFrame& frame) {
    require_mode(PortTunnelMode::Connect, PortTunnelProtocol::Tcp, "tcp connect requires an open tcp connect tunnel");

    const std::string endpoint = ensure_nonzero_connect_endpoint(frame_meta_string(frame, "endpoint"));

    if (!service_->try_acquire_active_tcp_stream()) {
        throw PortForwardError(400, "port_tunnel_limit_exceeded", "port tunnel active tcp stream limit reached");
    }

    std::shared_ptr<TunnelTcpStream> stream;
    try {
        UniqueSocket connected_socket(
            connect_port_forward_socket(endpoint, "tcp", service_->limits().connect_timeout_ms));
        stream.reset(new TunnelTcpStream(connected_socket.release(), service_, true));
    } catch (...) {
        service_->release_active_tcp_stream();
        throw;
    }

    connection_local_streams_.insert_tcp(frame.stream_id, stream);
    if (!service_->try_acquire_worker()) {
        drop_tcp_stream(frame.stream_id, stream);
        send_worker_limit(frame.stream_id);
        return;
    }
    if (!send_tcp_success_after_io_threads_started(
            make_empty_frame(PortTunnelFrameType::TcpConnectOk, frame.stream_id), frame.stream_id, stream, true)) {
        send_worker_limit(frame.stream_id);
        return;
    }
}

void PortTunnelConnection::tcp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        const int received =
            recv(stream->socket.get(), reinterpret_cast<char*>(buffer.data()), static_cast<int>(buffer.size()), 0);
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
        if (!send_data_frame_or_limit_error(frame)) {
            mark_tcp_stream_closed(stream);
            return;
        }
    }
}

void PortTunnelConnection::tcp_write_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream) {
    for (;;) {
        std::vector<unsigned char> data;
        {
            BasicLockGuard lock(stream->mutex);
            while (!stream->closed && stream->write_queue.empty() && !stream->writer_closed &&
                   !stream->writer_shutdown_requested) {
                stream->writer_cond.wait(stream->mutex);
            }
            if (stream->closed || stream->writer_closed) {
                return;
            }
            if (stream->write_queue.empty() && stream->writer_shutdown_requested) {
                stream->writer_closed = true;
                break;
            }
            data.swap(stream->write_queue.front());
            stream->write_queue.erase(stream->write_queue.begin());
            stream->writer_cond.signal();
        }
        try {
            send_all_bytes(stream->socket.get(), reinterpret_cast<const char*>(data.data()), data.size());
        } catch (const std::exception& ex) {
            if (!tcp_stream_closed(stream)) {
                send_error(stream_id, "port_write_failed", ex.what());
            }
            mark_tcp_stream_closed(stream);
            return;
        }
    }
#ifdef _WIN32
    shutdown(stream->socket.get(), SD_SEND);
#else
    shutdown(stream->socket.get(), SHUT_WR);
#endif
}

void PortTunnelConnection::tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data) {
    std::shared_ptr<TunnelTcpStream> stream;
    stream = connection_local_streams_.get_tcp(stream_id);
    if (stream.get() == NULL) {
        throw PortForwardError(400, "unknown_port_connection", "unknown tunnel tcp stream");
    }
    {
        BasicLockGuard lock(stream->mutex);
        if (stream->closed || stream->writer_closed || stream->writer_shutdown_requested) {
            throw PortForwardError(400, "port_connection_closed", "connection was closed");
        }
        if (stream->write_queue.size() >= TCP_WRITE_QUEUE_LIMIT) {
            stream->writer_closed = true;
            stream->writer_cond.broadcast();
        } else {
            stream->write_queue.push_back(data);
            stream->writer_cond.signal();
            return;
        }
    }
    mark_tcp_stream_closed(stream);
    throw PortForwardError(400, "port_tunnel_limit_exceeded", "tcp write queue limit reached");
}

void PortTunnelConnection::tcp_eof(uint32_t stream_id) {
    std::shared_ptr<TunnelTcpStream> stream;
    stream = connection_local_streams_.get_tcp(stream_id);
    if (stream.get() == NULL) {
        return;
    }
    {
        BasicLockGuard lock(stream->mutex);
        stream->writer_shutdown_requested = true;
        stream->writer_cond.broadcast();
    }
}
