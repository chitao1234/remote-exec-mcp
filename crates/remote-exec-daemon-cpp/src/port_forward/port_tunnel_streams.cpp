#include "port_tunnel_streams.h"

namespace {

bool resource_state_is_unavailable(PortTunnelResourceState state) {
    return state != PortTunnelResourceState::Open;
}

bool begin_resource_close_locked(PortTunnelResourceState* state) {
    if (resource_state_is_unavailable(*state)) {
        return false;
    }
    *state = PortTunnelResourceState::Closing;
    return true;
}

void finish_resource_close_locked(PortTunnelResourceState* state) {
    *state = PortTunnelResourceState::Closed;
}

} // namespace

const char* port_tunnel_resource_state_name(PortTunnelResourceState state) {
    switch (state) {
    case PortTunnelResourceState::Open:
        return "open";
    case PortTunnelResourceState::Closing:
        return "closing";
    case PortTunnelResourceState::Closed:
        return "closed";
    }
    return "unknown";
}

void TunnelTcpStream::close() {
    BasicLockGuard lock(mutex);
    if (!begin_resource_close_locked(&resource_state)) {
        return;
    }
    writer_closed = true;
    writer_shutdown_requested = true;
    write_queue.clear();
    writer_cond.broadcast();
    shutdown_socket(socket.get());
    socket.reset();
    active_stream_budget.reset();
    finish_resource_close_locked(&resource_state);
}

bool TunnelTcpStream::is_closed() {
    BasicLockGuard lock(mutex);
    return is_closing_or_closed_locked();
}

bool TunnelTcpStream::is_closing_or_closed_locked() const {
    return resource_state_is_unavailable(resource_state);
}

PortTunnelResourceState TunnelTcpStream::resource_state_snapshot() {
    BasicLockGuard lock(mutex);
    return resource_state;
}

void TunnelUdpSocket::close() {
    BasicLockGuard lock(mutex);
    if (!begin_resource_close_locked(&resource_state)) {
        return;
    }
    wakeup.signal();
    shutdown_socket(socket.get());
    socket.reset();
    udp_bind_budget.reset();
    finish_resource_close_locked(&resource_state);
}

bool TunnelUdpSocket::is_closed() {
    BasicLockGuard lock(mutex);
    return is_closing_or_closed_locked();
}

bool TunnelUdpSocket::is_closing_or_closed_locked() const {
    return resource_state_is_unavailable(resource_state);
}

PortTunnelResourceState TunnelUdpSocket::resource_state_snapshot() {
    BasicLockGuard lock(mutex);
    return resource_state;
}

void RetainedTcpListener::close() {
    BasicLockGuard lock(mutex);
    if (!begin_resource_close_locked(&resource_state)) {
        return;
    }
    wakeup.signal();
    shutdown_socket(listener.get());
    listener.reset();
    retained_listener_budget.reset();
    finish_resource_close_locked(&resource_state);
}

bool RetainedTcpListener::is_closed() {
    BasicLockGuard lock(mutex);
    return is_closing_or_closed_locked();
}

bool RetainedTcpListener::is_closing_or_closed_locked() const {
    return resource_state_is_unavailable(resource_state);
}

PortTunnelResourceState RetainedTcpListener::resource_state_snapshot() {
    BasicLockGuard lock(mutex);
    return resource_state;
}

void ConnectionLocalStreams::insert_tcp(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& stream) {
    BasicLockGuard lock(mutex_);
    tcp_streams_[stream_id] = stream;
}

std::shared_ptr<TunnelTcpStream> ConnectionLocalStreams::get_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>>::iterator it = tcp_streams_.find(stream_id);
    if (it == tcp_streams_.end()) {
        return std::shared_ptr<TunnelTcpStream>();
    }
    return it->second;
}

std::shared_ptr<TunnelTcpStream> ConnectionLocalStreams::remove_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>>::iterator it = tcp_streams_.find(stream_id);
    if (it == tcp_streams_.end()) {
        return std::shared_ptr<TunnelTcpStream>();
    }
    std::shared_ptr<TunnelTcpStream> stream = it->second;
    tcp_streams_.erase(it);
    return stream;
}

void ConnectionLocalStreams::insert_udp(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(mutex_);
    udp_sockets_[stream_id] = socket_value;
}

std::shared_ptr<TunnelUdpSocket> ConnectionLocalStreams::get_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator it = udp_sockets_.find(stream_id);
    if (it == udp_sockets_.end()) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    return it->second;
}

std::shared_ptr<TunnelUdpSocket> ConnectionLocalStreams::remove_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator it = udp_sockets_.find(stream_id);
    if (it == udp_sockets_.end()) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    std::shared_ptr<TunnelUdpSocket> socket_value = it->second;
    udp_sockets_.erase(it);
    return socket_value;
}

void ConnectionLocalStreams::drain(
    std::vector<std::shared_ptr<TunnelTcpStream>>* tcp_streams,
    std::vector<std::shared_ptr<TunnelUdpSocket>>* udp_sockets
) {
    BasicLockGuard lock(mutex_);
    for (std::map<uint32_t, std::shared_ptr<TunnelTcpStream>>::iterator it = tcp_streams_.begin();
         it != tcp_streams_.end();
         ++it) {
        tcp_streams->push_back(it->second);
    }
    tcp_streams_.clear();
    for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator it = udp_sockets_.begin();
         it != udp_sockets_.end();
         ++it) {
        udp_sockets->push_back(it->second);
    }
    udp_sockets_.clear();
}
