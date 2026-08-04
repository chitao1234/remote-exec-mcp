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

template <typename T>
std::shared_ptr<T> map_find(std::map<uint32_t, std::shared_ptr<T>>& streams, uint32_t stream_id) {
    typename std::map<uint32_t, std::shared_ptr<T>>::iterator it = streams.find(stream_id);
    if (it == streams.end()) {
        return std::shared_ptr<T>();
    }
    return it->second;
}

template <typename T>
std::shared_ptr<T> map_remove(std::map<uint32_t, std::shared_ptr<T>>& streams, uint32_t stream_id) {
    typename std::map<uint32_t, std::shared_ptr<T>>::iterator it = streams.find(stream_id);
    if (it == streams.end()) {
        return std::shared_ptr<T>();
    }
    std::shared_ptr<T> stream = it->second;
    streams.erase(it);
    return stream;
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

void ConnectionLocalStreams::insert_tcp(
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream
) {
    BasicLockGuard lock(mutex_);
    tcp_streams_[stream_id] = stream;
}

std::shared_ptr<TunnelTcpStream> ConnectionLocalStreams::get_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    return map_find(tcp_streams_, stream_id);
}

std::shared_ptr<TunnelTcpStream> ConnectionLocalStreams::remove_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    return map_remove(tcp_streams_, stream_id);
}

void ConnectionLocalStreams::insert_udp(
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
    BasicLockGuard lock(mutex_);
    udp_sockets_[stream_id] = socket_value;
}

std::shared_ptr<TunnelUdpSocket> ConnectionLocalStreams::get_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    return map_find(udp_sockets_, stream_id);
}

std::shared_ptr<TunnelUdpSocket> ConnectionLocalStreams::remove_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    return map_remove(udp_sockets_, stream_id);
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
