#pragma once

#include <memory>

#include "port_tunnel_streams.h"

class PortTunnelConnection;
class PortTunnelService;

class TcpReadStartGate {
public:
    TcpReadStartGate();

    void release();
    void wait();

private:
    TcpReadStartGate(const TcpReadStartGate&);
    TcpReadStartGate& operator=(const TcpReadStartGate&);

    BasicMutex mutex_;
    BasicCondVar cond_;
    bool released_;
};

bool spawn_tcp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelTcpStream>& stream,
                           bool worker_acquired = false,
                           const std::shared_ptr<TcpReadStartGate>& start_gate = std::shared_ptr<TcpReadStartGate>());
bool spawn_tcp_write_thread(const std::shared_ptr<PortTunnelService>& service,
                            const std::shared_ptr<PortTunnelConnection>& tunnel,
                            uint32_t stream_id,
                            const std::shared_ptr<TunnelTcpStream>& stream,
                            bool worker_acquired = false);
bool spawn_udp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelUdpSocket>& socket_value,
                           bool worker_acquired = false);
