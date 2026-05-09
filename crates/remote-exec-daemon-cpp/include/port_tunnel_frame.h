#pragma once

#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>

static const std::size_t PORT_TUNNEL_HEADER_LEN = 16U;
static const std::size_t PORT_TUNNEL_MAX_META_LEN = 16U * 1024U;
static const std::size_t PORT_TUNNEL_MAX_DATA_LEN = 256U * 1024U;

enum class PortTunnelFrameType : unsigned char {
    Error = 1,
    Close = 2,
    SessionOpen = 3,
    SessionReady = 4,
    SessionResume = 5,
    SessionResumed = 6,
    TunnelOpen = 7,
    TunnelReady = 8,
    TunnelClose = 9,
    TcpListen = 10,
    TcpListenOk = 11,
    TcpAccept = 12,
    TcpConnect = 13,
    TcpConnectOk = 14,
    TcpData = 15,
    TcpEof = 16,
    TunnelClosed = 17,
    TunnelHeartbeat = 18,
    TunnelHeartbeatAck = 19,
    ForwardRecovering = 20,
    ForwardRecovered = 21,
    ForwardDrop = 22,
    UdpBind = 30,
    UdpBindOk = 31,
    UdpDatagram = 32
};

struct PortTunnelFrame {
    PortTunnelFrameType type;
    unsigned char flags;
    uint32_t stream_id;
    std::string meta;
    std::vector<unsigned char> data;
};

class PortTunnelFrameError : public std::runtime_error {
public:
    explicit PortTunnelFrameError(const std::string& message);
};

const char* port_tunnel_preface();
std::size_t port_tunnel_preface_size();

std::vector<unsigned char> encode_port_tunnel_frame(const PortTunnelFrame& frame);
PortTunnelFrame decode_port_tunnel_frame(const std::vector<unsigned char>& bytes);
