#include "port_tunnel_connection.h"
#include "port_tunnel_spawn.h"
#include "port_tunnel_service.h"

bool PortTunnelService::spawn_udp_bind_loop(const std::shared_ptr<PortTunnelSession>& session,
                                            uint32_t stream_id,
                                            const std::shared_ptr<TunnelUdpSocket>& socket_value,
                                            bool worker_acquired) {
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    if (!worker_acquired && !service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
        uint32_t stream_id;
        std::shared_ptr<TunnelUdpSocket> socket_value;
    };

    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            PortTunnelWorkerLease lease(context->service);
            context->service->udp_read_loop(context->session, context->stream_id, context->socket_value);
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = service;
    context->session = session;
    context->stream_id = stream_id;
    context->socket_value = socket_value;
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
        std::thread([service, session, stream_id, socket_value]() {
            PortTunnelWorkerLease lease(service);
            service->udp_read_loop(session, stream_id, socket_value);
        }).detach();
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn udp bind thread", ex);
        service->release_worker();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn udp bind thread");
        service->release_worker();
        return false;
    }
    return true;
#endif
}

void PortTunnelService::udp_read_loop(const std::shared_ptr<PortTunnelSession>& session,
                                      uint32_t stream_id,
                                      const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        std::shared_ptr<PortTunnelSessionAttachment> attachment = wait_for_attachment(session);
        if (attachment.get() == NULL) {
            return;
        }
        std::shared_ptr<PortTunnelConnection> connection = attachment->connection.lock();
        if (connection.get() == NULL) {
            continue;
        }

        const int ready = wait_socket_readable(socket_value->socket.get(), RETAINED_SOCKET_POLL_TIMEOUT_MS);
        if (ready == 0) {
            continue;
        }
        if (ready < 0) {
            if (udp_socket_closed(socket_value) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(stream_id, "port_read_failed", socket_error_message("select"));
            }
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }

        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const int received = recvfrom(socket_value->socket.get(),
                                      reinterpret_cast<char*>(buffer.data()),
                                      static_cast<int>(buffer.size()),
                                      0,
                                      reinterpret_cast<sockaddr*>(&peer_address),
                                      &peer_len);
        if (received < 0) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                continue;
            }
            if (udp_socket_closed(socket_value) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(stream_id, "port_read_failed", socket_error_message("recvfrom"));
            }
            return;
        }
        if (udp_socket_closed(socket_value) || session_is_unavailable(session)) {
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }
        std::vector<unsigned char> payload(buffer.begin(), buffer.begin() + received);
        if (!connection->emit_session_udp_datagram(
                session,
                stream_id,
                printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len),
                payload)) {
            if (session_is_unavailable(session)) {
                return;
            }
        }
    }
}

void PortTunnelConnection::udp_bind(const PortTunnelFrame& frame) {
    const PortTunnelMode mode = current_mode();
    if (mode == PortTunnelMode::Listen) {
        require_mode(PortTunnelMode::Listen, PortTunnelProtocol::Udp, "udp bind requires an open udp listen tunnel");
    } else {
        require_mode(PortTunnelMode::Connect, PortTunnelProtocol::Udp, "udp bind requires an open udp connect tunnel");
    }

    const std::string endpoint = normalize_port_forward_endpoint(frame_meta_string(frame, "endpoint"));
    if (!service_->try_acquire_udp_bind()) {
        throw PortForwardError(400, "port_tunnel_limit_exceeded", "port tunnel udp bind limit reached");
    }

    std::shared_ptr<TunnelUdpSocket> socket_value;
    try {
        socket_value.reset(new TunnelUdpSocket(bind_port_forward_socket(endpoint, "udp"), service_, true));
    } catch (const std::exception& ex) {
        log_tunnel_exception("create udp bind socket", ex);
        service_->release_udp_bind();
        throw;
    } catch (...) {
        log_unknown_tunnel_exception("create udp bind socket");
        service_->release_udp_bind();
        throw;
    }
    const std::string bound_endpoint = socket_local_endpoint(socket_value->socket.get());

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        if (!service_->try_acquire_worker()) {
            mark_udp_socket_closed(socket_value);
            send_worker_limit(frame.stream_id);
            return;
        }
        {
            BasicLockGuard lock(session->mutex);
            session->udp_binds[frame.stream_id] = socket_value;
        }
        if (!service_->spawn_udp_bind_loop(session, frame.stream_id, socket_value, true)) {
            {
                BasicLockGuard lock(session->mutex);
                session->udp_binds.erase(frame.stream_id);
            }
            mark_udp_socket_closed(socket_value);
            send_worker_limit(frame.stream_id);
            return;
        }
    } else {
        if (!service_->try_acquire_worker()) {
            mark_udp_socket_closed(socket_value);
            send_worker_limit(frame.stream_id);
            return;
        }
        connection_local_streams_.insert_udp(frame.stream_id, socket_value);
        if (!spawn_udp_read_thread(service_, shared_from_this(), frame.stream_id, socket_value, true)) {
            std::shared_ptr<TunnelUdpSocket> removed_socket = connection_local_streams_.remove_udp(frame.stream_id);
            if (removed_socket.get() != NULL) {
                mark_udp_socket_closed(removed_socket);
            } else {
                mark_udp_socket_closed(socket_value);
            }
            send_worker_limit(frame.stream_id);
            return;
        }
    }

    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::UdpBindOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
}

void PortTunnelConnection::udp_read_loop_connection_local(uint32_t stream_id,
                                                          std::shared_ptr<TunnelUdpSocket> socket_value) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const int received = recvfrom(socket_value->socket.get(),
                                      reinterpret_cast<char*>(buffer.data()),
                                      static_cast<int>(buffer.size()),
                                      0,
                                      reinterpret_cast<sockaddr*>(&peer_address),
                                      &peer_len);
        if (received < 0) {
            if (!udp_socket_closed(socket_value)) {
                send_error(stream_id, "port_read_failed", socket_error_message("recvfrom"));
            }
            return;
        }
        if (udp_socket_closed(socket_value)) {
            return;
        }
        PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::UdpDatagram, stream_id);
        frame.meta =
            Json{{"peer", printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)}}
                .dump();
        frame.data.assign(buffer.begin(), buffer.begin() + received);
        if (!send_data_frame_or_drop_on_limit(frame)) {
            continue;
        }
    }
}

void PortTunnelConnection::udp_datagram(const PortTunnelFrame& frame) {
    const PortTunnelMode mode = current_mode();
    if (mode == PortTunnelMode::Listen) {
        require_mode(PortTunnelMode::Listen, PortTunnelProtocol::Udp, "udp datagram requires an open udp tunnel");
    } else {
        require_mode(PortTunnelMode::Connect, PortTunnelProtocol::Udp, "udp datagram requires an open udp tunnel");
    }

    std::shared_ptr<TunnelUdpSocket> socket_value;
    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        BasicLockGuard lock(session->mutex);
        std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator it = session->udp_binds.find(frame.stream_id);
        if (it == session->udp_binds.end()) {
            throw PortForwardError(400, "unknown_port_bind", "unknown tunnel udp stream");
        }
        socket_value = it->second;
    } else {
        socket_value = connection_local_streams_.get_udp(frame.stream_id);
        if (socket_value.get() == NULL) {
            throw PortForwardError(400, "unknown_port_bind", "unknown tunnel udp stream");
        }
    }
    const std::string peer = frame_meta_string(frame, "peer");
    socklen_t peer_len = 0;
    const sockaddr_storage peer_address = parse_port_forward_peer(peer, &peer_len);
    const int sent = sendto(socket_value->socket.get(),
                            reinterpret_cast<const char*>(frame.data.data()),
                            static_cast<int>(frame.data.size()),
                            0,
                            reinterpret_cast<const sockaddr*>(&peer_address),
                            peer_len);
    if (sent < 0 || static_cast<std::size_t>(sent) != frame.data.size()) {
        throw PortForwardError(400, "port_write_failed", socket_error_message("sendto"));
    }
}
