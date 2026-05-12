#include "port_tunnel_streams.h"

void TransportOwnedStreams::insert_tcp(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& stream) {
    BasicLockGuard lock(mutex_);
    tcp_streams_[stream_id] = stream;
}

std::shared_ptr<TunnelTcpStream> TransportOwnedStreams::get_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>>::iterator it = tcp_streams_.find(stream_id);
    if (it == tcp_streams_.end()) {
        return std::shared_ptr<TunnelTcpStream>();
    }
    return it->second;
}

std::shared_ptr<TunnelTcpStream> TransportOwnedStreams::remove_tcp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>>::iterator it = tcp_streams_.find(stream_id);
    if (it == tcp_streams_.end()) {
        return std::shared_ptr<TunnelTcpStream>();
    }
    std::shared_ptr<TunnelTcpStream> stream = it->second;
    tcp_streams_.erase(it);
    return stream;
}

void TransportOwnedStreams::insert_udp(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(mutex_);
    udp_sockets_[stream_id] = socket_value;
}

std::shared_ptr<TunnelUdpSocket> TransportOwnedStreams::get_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator it = udp_sockets_.find(stream_id);
    if (it == udp_sockets_.end()) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    return it->second;
}

std::shared_ptr<TunnelUdpSocket> TransportOwnedStreams::remove_udp(uint32_t stream_id) {
    BasicLockGuard lock(mutex_);
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>>::iterator it = udp_sockets_.find(stream_id);
    if (it == udp_sockets_.end()) {
        return std::shared_ptr<TunnelUdpSocket>();
    }
    std::shared_ptr<TunnelUdpSocket> socket_value = it->second;
    udp_sockets_.erase(it);
    return socket_value;
}

void TransportOwnedStreams::drain(std::vector<std::shared_ptr<TunnelTcpStream>>* tcp_streams,
                                  std::vector<std::shared_ptr<TunnelUdpSocket>>* udp_sockets) {
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
