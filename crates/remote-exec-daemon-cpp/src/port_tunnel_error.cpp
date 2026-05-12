#include <sstream>

#include "port_tunnel_connection.h"
#include "port_tunnel_spawn.h"
#include "port_tunnel_service.h"

void PortTunnelConnection::send_error(uint32_t stream_id, const std::string& code, const std::string& message) {
    PortTunnelFrame frame;
    frame.type = PortTunnelFrameType::Error;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta =
        Json{{"code", code}, {"message", message}, {"fatal", false}, {"generation", current_generation()}}.dump();
    try {
        send_frame(frame);
    } catch (const std::exception&) {
    }
}

void PortTunnelConnection::send_terminal_error(uint32_t stream_id,
                                               const std::string& code,
                                               const std::string& message) {
    PortTunnelFrame frame;
    frame.type = PortTunnelFrameType::Error;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta =
        Json{{"code", code}, {"message", message}, {"fatal", true}, {"generation", current_generation()}}.dump();
    try {
        send_frame(frame);
    } catch (const std::exception&) {
    }
}

void PortTunnelConnection::send_forward_drop(uint32_t stream_id,
                                             const std::string& kind,
                                             const std::string& reason,
                                             const std::string& message) {
    PortTunnelFrame frame;
    frame.type = PortTunnelFrameType::ForwardDrop;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta = Json{{"kind", kind}, {"count", 1U}, {"reason", reason}, {"message", message}}.dump();
    try {
        send_frame(frame);
    } catch (const std::exception&) {
    }
}

void PortTunnelConnection::close_stream(uint32_t stream_id) {
    std::shared_ptr<TunnelTcpStream> tcp_stream = transport_streams_.remove_tcp(stream_id);
    if (tcp_stream.get() != NULL) {
        mark_tcp_stream_closed(tcp_stream);
    }

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        bool close_session_now = false;
        {
            BasicLockGuard lock(session->mutex);
            std::map<uint32_t, std::shared_ptr<RetainedTcpListener>>::iterator listener =
                session->tcp_listeners.find(stream_id);
            if (listener != session->tcp_listeners.end()) {
                mark_retained_listener_closed(listener->second);
                session->tcp_listeners.erase(listener);
                close_session_now = true;
            }
            std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator udp = session->udp_binds.find(stream_id);
            if (udp != session->udp_binds.end()) {
                mark_udp_socket_closed(udp->second);
                session->udp_binds.erase(udp);
                close_session_now = true;
            }
        }
        if (close_session_now) {
            service_->close_session(session);
        }
        send_frame(make_empty_frame(PortTunnelFrameType::Close, stream_id));
        return;
    }

    std::shared_ptr<TunnelUdpSocket> udp_socket = transport_streams_.remove_udp(stream_id);
    if (udp_socket.get() != NULL) {
        mark_udp_socket_closed(udp_socket);
    }
    send_frame(make_empty_frame(PortTunnelFrameType::Close, stream_id));
}

void PortTunnelConnection::send_worker_limit(uint32_t stream_id) {
    send_error(stream_id, "port_tunnel_limit_exceeded", "port tunnel worker limit reached");
}

void PortTunnelConnection::drop_tcp_stream(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& fallback) {
    std::shared_ptr<TunnelTcpStream> removed_stream = transport_streams_.remove_tcp(stream_id);
    if (removed_stream.get() != NULL) {
        mark_tcp_stream_closed(removed_stream);
    } else if (fallback.get() != NULL) {
        mark_tcp_stream_closed(fallback);
    }
}

bool PortTunnelConnection::send_tcp_success_after_io_threads_started(const PortTunnelFrame& success,
                                                                     uint32_t stream_id,
                                                                     const std::shared_ptr<TunnelTcpStream>& stream,
                                                                     bool worker_acquired) {
    std::shared_ptr<TcpReadStartGate> start_gate(new TcpReadStartGate());
    if (!spawn_tcp_write_thread(service_, shared_from_this(), stream_id, stream, false)) {
        drop_tcp_stream(stream_id, stream);
        if (worker_acquired) {
            service_->release_worker();
        }
        return false;
    }
    if (!spawn_tcp_read_thread(service_, shared_from_this(), stream_id, stream, worker_acquired, start_gate)) {
        drop_tcp_stream(stream_id, stream);
        return false;
    }
    send_frame(success);
    start_gate->release();
    return true;
}

void PortTunnelConnection::close_current_session(PortTunnelCloseMode mode) {
    std::shared_ptr<PortTunnelSession> session = current_session();
    if (session.get() != NULL) {
        if (mode == PortTunnelCloseMode::RetryableDetach) {
            service_->detach_session(session);
        } else if (mode == PortTunnelCloseMode::GracefulClose || mode == PortTunnelCloseMode::TerminalFailure) {
            service_->close_session(session);
        }
    }
}

void PortTunnelConnection::close_transport_owned_state() {
    std::vector<std::shared_ptr<TunnelTcpStream>> tcp_streams;
    std::vector<std::shared_ptr<TunnelUdpSocket>> udp_sockets;
    mark_closed();
    transport_streams_.drain(&tcp_streams, &udp_sockets);
    for (std::size_t i = 0; i < tcp_streams.size(); ++i) {
        mark_tcp_stream_closed(tcp_streams[i]);
    }
    for (std::size_t i = 0; i < udp_sockets.size(); ++i) {
        mark_udp_socket_closed(udp_sockets[i]);
    }
}

std::shared_ptr<PortTunnelSession> PortTunnelConnection::current_session() {
    BasicLockGuard lock(state_mutex_);
    return session_;
}

PortTunnelMode PortTunnelConnection::current_mode() {
    BasicLockGuard lock(state_mutex_);
    return mode_;
}

void PortTunnelConnection::require_mode(PortTunnelMode mode, PortTunnelProtocol protocol, const std::string& message) {
    BasicLockGuard lock(state_mutex_);
    if (mode_ != mode || protocol_ != protocol) {
        throw PortForwardError(400, "invalid_port_tunnel", message);
    }
}

bool PortTunnelConnection::session_mode_active() {
    return current_session().get() != NULL;
}

std::uint64_t PortTunnelConnection::current_generation() const {
    return generation_.load();
}

void PortTunnelConnection::set_generation(std::uint64_t generation) {
    generation_.store(generation);
}

void PortTunnelConnection::ensure_generation(std::uint64_t frame_generation) const {
    const std::uint64_t generation = current_generation();
    if (frame_generation != generation) {
        std::ostringstream message;
        message << "frame generation `" << frame_generation << "` does not match tunnel generation `" << generation
                << "`";
        throw PortForwardError(400, "port_tunnel_generation_mismatch", message.str());
    }
}

bool PortTunnelConnection::owns_session(const std::shared_ptr<PortTunnelSession>& session) {
    BasicLockGuard lock(state_mutex_);
    return !closed() && session_.get() == session.get();
}

bool PortTunnelConnection::accept_session_tcp_stream(const std::shared_ptr<PortTunnelSession>& session,
                                                     uint32_t listener_stream_id,
                                                     UniqueSocket accepted_socket,
                                                     const std::string& peer) {
    uint32_t stream_id = 0U;
    bool worker_acquired = false;
    if (!service_->try_acquire_active_tcp_stream()) {
        send_forward_drop(listener_stream_id,
                          "tcp_stream",
                          "port_tunnel_limit_exceeded",
                          "port tunnel active tcp stream limit reached");
        return false;
    }

    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired) {
            service_->release_active_tcp_stream();
            return false;
        }
        stream_id = session->next_daemon_stream_id;
        session->next_daemon_stream_id += 2U;
    }

    if (!service_->try_acquire_worker()) {
        service_->release_active_tcp_stream();
        send_forward_drop(
            listener_stream_id, "tcp_stream", "port_tunnel_limit_exceeded", "port tunnel worker limit reached");
        return false;
    }
    worker_acquired = true;

    std::shared_ptr<TunnelTcpStream> stream(new TunnelTcpStream(accepted_socket.release(), service_, true));
    {
        BasicLockGuard lock(state_mutex_);
        if (closed() || session_.get() != session.get()) {
            mark_tcp_stream_closed(stream);
            service_->release_worker();
            return false;
        }
        transport_streams_.insert_tcp(stream_id, stream);
    }

    PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpAccept, stream_id);
    frame.meta = Json{{"listener_stream_id", listener_stream_id}, {"peer", peer}}.dump();
    if (!owns_session(session) || closed()) {
        drop_tcp_stream(stream_id, stream);
        service_->release_worker();
        return false;
    }

    if (!send_tcp_success_after_io_threads_started(frame, stream_id, stream, worker_acquired)) {
        return false;
    }
    return true;
}

bool PortTunnelConnection::emit_session_udp_datagram(const std::shared_ptr<PortTunnelSession>& session,
                                                     uint32_t stream_id,
                                                     const std::string& peer,
                                                     const std::vector<unsigned char>& data) {
    if (!owns_session(session)) {
        return false;
    }
    PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::UdpDatagram, stream_id);
    frame.meta = Json{{"peer", peer}}.dump();
    frame.data = data;
    if (!send_data_frame_or_drop_on_limit(frame)) {
        return false;
    }
    return owns_session(session) && !closed();
}
