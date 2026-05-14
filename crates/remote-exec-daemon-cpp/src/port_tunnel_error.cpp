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
    std::shared_ptr<TunnelTcpStream> tcp_stream = remove_active_tcp_stream(stream_id);
    if (tcp_stream.get() != nullptr) {
        mark_tcp_stream_closed(tcp_stream);
    }

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        if (service_->close_session_retained_resource(session, stream_id)) {
            service_->close_session(session);
        }
        send_frame(make_empty_frame(PortTunnelFrameType::Close, stream_id));
        return;
    }

    std::shared_ptr<TunnelUdpSocket> udp_socket = connection_local_streams_.remove_udp(stream_id);
    if (udp_socket.get() != nullptr) {
        mark_udp_socket_closed(udp_socket);
    }
    send_frame(make_empty_frame(PortTunnelFrameType::Close, stream_id));
}

void PortTunnelConnection::send_worker_limit(uint32_t stream_id) {
    send_error(stream_id, "port_tunnel_limit_exceeded", "port tunnel worker limit reached");
}

void PortTunnelConnection::drop_tcp_stream(ConnectionLocalStreams* local_streams,
                                           uint32_t stream_id,
                                           const std::shared_ptr<TunnelTcpStream>& fallback) {
    std::shared_ptr<TunnelTcpStream> removed_stream = local_streams->remove_tcp(stream_id);
    if (removed_stream.get() != nullptr) {
        mark_tcp_stream_closed(removed_stream);
    } else if (fallback.get() != nullptr) {
        mark_tcp_stream_closed(fallback);
    }
}

bool PortTunnelConnection::send_tcp_success_after_io_threads_started(const PortTunnelFrame& success,
                                                                     ConnectionLocalStreams* local_streams,
                                                                     uint32_t stream_id,
                                                                     const std::shared_ptr<TunnelTcpStream>& stream,
                                                                     bool worker_acquired) {
    std::shared_ptr<TcpReadStartGate> start_gate(new TcpReadStartGate());
    if (!spawn_tcp_write_thread(service_, shared_from_this(), stream_id, stream, false)) {
        drop_tcp_stream(local_streams, stream_id, stream);
        if (worker_acquired) {
            service_->release_worker();
        }
        return false;
    }
    if (!spawn_tcp_read_thread(service_, shared_from_this(), stream_id, stream, worker_acquired, start_gate)) {
        drop_tcp_stream(local_streams, stream_id, stream);
        return false;
    }
    send_frame(success);
    start_gate->release();
    return true;
}

void PortTunnelConnection::close_current_session(PortTunnelCloseMode mode) {
    std::shared_ptr<PortTunnelSession> session = current_session();
    if (session.get() != nullptr) {
        if (mode == PortTunnelCloseMode::RetryableDetach) {
            service_->detach_session(session);
        } else if (mode == PortTunnelCloseMode::GracefulClose || mode == PortTunnelCloseMode::TerminalFailure) {
            service_->close_session(session);
        }
    }
}

void PortTunnelConnection::close_connection_local_state() {
    std::vector<std::shared_ptr<TunnelTcpStream>> tcp_streams;
    std::vector<std::shared_ptr<TunnelUdpSocket>> udp_sockets;
    mark_closed();
    connection_local_streams_.drain(&tcp_streams, &udp_sockets);
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

std::shared_ptr<PortTunnelSessionAttachment> PortTunnelConnection::session_attachment_for(
    const std::shared_ptr<PortTunnelSession>& session) {
    if (session.get() == nullptr) {
        return std::shared_ptr<PortTunnelSessionAttachment>();
    }
    BasicLockGuard lock(session->mutex);
    return session->attachment;
}

std::shared_ptr<PortTunnelSessionAttachment> PortTunnelConnection::current_session_attachment() {
    return session_attachment_for(current_session());
}

std::shared_ptr<TunnelTcpStream> PortTunnelConnection::get_active_tcp_stream(uint32_t stream_id) {
    std::shared_ptr<PortTunnelSessionAttachment> attachment = current_session_attachment();
    if (attachment.get() != nullptr) {
        return attachment->local_streams.get_tcp(stream_id);
    }
    return connection_local_streams_.get_tcp(stream_id);
}

std::shared_ptr<TunnelTcpStream> PortTunnelConnection::remove_active_tcp_stream(uint32_t stream_id) {
    std::shared_ptr<PortTunnelSessionAttachment> attachment = current_session_attachment();
    if (attachment.get() != nullptr) {
        return attachment->local_streams.remove_tcp(stream_id);
    }
    return connection_local_streams_.remove_tcp(stream_id);
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
    return current_session().get() != nullptr;
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

bool PortTunnelConnection::owns_attachment(const std::shared_ptr<PortTunnelSessionAttachment>& attachment) {
    if (attachment.get() == nullptr) {
        return false;
    }
    std::shared_ptr<PortTunnelSession> session;
    {
        BasicLockGuard lock(state_mutex_);
        if (closed() || session_.get() == nullptr) {
            return false;
        }
        session = session_;
    }
    return session_attachment_for(session).get() == attachment.get();
}

bool PortTunnelConnection::accept_session_tcp_stream(const std::shared_ptr<PortTunnelSession>& session,
                                                     const std::shared_ptr<PortTunnelSessionAttachment>& attachment,
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

    std::shared_ptr<TunnelTcpStream> stream(new TunnelTcpStream(accepted_socket.release(), service_, true));
    {
        BasicLockGuard state_lock(state_mutex_);
        if (closed() || session_.get() != session.get()) {
            mark_tcp_stream_closed(stream);
            return false;
        }
        BasicLockGuard session_lock(session->mutex);
        if (session->closed || session->expired || session->attachment.get() != attachment.get()) {
            mark_tcp_stream_closed(stream);
            return false;
        }
        stream_id = session->next_daemon_stream_id;
        session->next_daemon_stream_id += 2U;
        attachment->local_streams.insert_tcp(stream_id, stream);
    }

    if (!service_->try_acquire_worker()) {
        attachment->local_streams.remove_tcp(stream_id);
        mark_tcp_stream_closed(stream);
        send_forward_drop(
            listener_stream_id, "tcp_stream", "port_tunnel_limit_exceeded", "port tunnel worker limit reached");
        return false;
    }
    worker_acquired = true;

    PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpAccept, stream_id);
    frame.meta = Json{{"listener_stream_id", listener_stream_id}, {"peer", peer}}.dump();
    if (!owns_attachment(attachment) || closed()) {
        drop_tcp_stream(&attachment->local_streams, stream_id, stream);
        service_->release_worker();
        return false;
    }

    if (!send_tcp_success_after_io_threads_started(frame, &attachment->local_streams, stream_id, stream, worker_acquired)) {
        return false;
    }
    return true;
}

bool PortTunnelConnection::emit_session_udp_datagram(const std::shared_ptr<PortTunnelSessionAttachment>& attachment,
                                                     uint32_t stream_id,
                                                     const std::string& peer,
                                                     const std::vector<unsigned char>& data) {
    if (!owns_attachment(attachment)) {
        return false;
    }
    PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::UdpDatagram, stream_id);
    frame.meta = Json{{"peer", peer}}.dump();
    frame.data = data;
    if (!send_data_frame_or_drop_on_limit(frame)) {
        return false;
    }
    return owns_attachment(attachment) && !closed();
}
