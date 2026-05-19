#include "port_tunnel_session_teardown.h"

namespace {

void close_connection_local_streams(ConnectionLocalStreams* local_streams) {
    std::vector<std::shared_ptr<TunnelTcpStream>> tcp_streams;
    std::vector<std::shared_ptr<TunnelUdpSocket>> udp_sockets;
    local_streams->drain(&tcp_streams, &udp_sockets);
    for (std::size_t i = 0; i < tcp_streams.size(); ++i) {
        tcp_streams[i]->close();
    }
    for (std::size_t i = 0; i < udp_sockets.size(); ++i) {
        udp_sockets[i]->close();
    }
}

} // namespace

void close_session_attachment(const std::shared_ptr<PortTunnelSessionAttachment>& attachment) {
    if (attachment.get() == nullptr) {
        return;
    }
    close_connection_local_streams(&attachment->local_streams);
}

void finish_terminal_session_teardown(const PortTunnelSessionTeardown& state) {
    close_session_attachment(state.attachment);
    if (state.retained_listener.get() != nullptr) {
        state.retained_listener->close();
    }
    if (state.udp_bind.get() != nullptr) {
        state.udp_bind->close();
    }
}

void close_retained_resource(const PortTunnelRetainedResource& resource) {
    if (resource.kind == PortTunnelRetainedResourceKind::TcpListener && resource.tcp_listener.get() != nullptr) {
        resource.tcp_listener->close();
    } else if (resource.kind == PortTunnelRetainedResourceKind::UdpBind && resource.udp_bind.get() != nullptr) {
        resource.udp_bind->close();
    }
}
