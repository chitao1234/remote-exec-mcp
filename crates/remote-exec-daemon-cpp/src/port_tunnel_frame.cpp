#include "port_tunnel_frame.h"

#include <limits>

namespace {

const char kPortTunnelPreface[] = {'R', 'E', 'P', 'F', 'W', 'D', '1', '\n'};

void write_u32_be(std::vector<unsigned char>& bytes, std::size_t offset, uint32_t value) {
    bytes[offset] = static_cast<unsigned char>((value >> 24) & 0xffU);
    bytes[offset + 1U] = static_cast<unsigned char>((value >> 16) & 0xffU);
    bytes[offset + 2U] = static_cast<unsigned char>((value >> 8) & 0xffU);
    bytes[offset + 3U] = static_cast<unsigned char>(value & 0xffU);
}

uint32_t read_u32_be(const std::vector<unsigned char>& bytes, std::size_t offset) {
    return (static_cast<uint32_t>(bytes[offset]) << 24) |
           (static_cast<uint32_t>(bytes[offset + 1U]) << 16) |
           (static_cast<uint32_t>(bytes[offset + 2U]) << 8) |
           static_cast<uint32_t>(bytes[offset + 3U]);
}

PortTunnelFrameType frame_type_from_byte(unsigned char value) {
    switch (value) {
        case 1:
            return PortTunnelFrameType::Error;
        case 2:
            return PortTunnelFrameType::Close;
        case 3:
            return PortTunnelFrameType::SessionOpen;
        case 4:
            return PortTunnelFrameType::SessionReady;
        case 5:
            return PortTunnelFrameType::SessionResume;
        case 6:
            return PortTunnelFrameType::SessionResumed;
        case 7:
            return PortTunnelFrameType::TunnelOpen;
        case 8:
            return PortTunnelFrameType::TunnelReady;
        case 9:
            return PortTunnelFrameType::TunnelClose;
        case 10:
            return PortTunnelFrameType::TcpListen;
        case 11:
            return PortTunnelFrameType::TcpListenOk;
        case 12:
            return PortTunnelFrameType::TcpAccept;
        case 13:
            return PortTunnelFrameType::TcpConnect;
        case 14:
            return PortTunnelFrameType::TcpConnectOk;
        case 15:
            return PortTunnelFrameType::TcpData;
        case 16:
            return PortTunnelFrameType::TcpEof;
        case 17:
            return PortTunnelFrameType::TunnelClosed;
        case 18:
            return PortTunnelFrameType::TunnelHeartbeat;
        case 19:
            return PortTunnelFrameType::TunnelHeartbeatAck;
        case 20:
            return PortTunnelFrameType::ForwardRecovering;
        case 21:
            return PortTunnelFrameType::ForwardRecovered;
        case 30:
            return PortTunnelFrameType::UdpBind;
        case 31:
            return PortTunnelFrameType::UdpBindOk;
        case 32:
            return PortTunnelFrameType::UdpDatagram;
        default:
            throw PortTunnelFrameError("unknown port tunnel frame type");
    }
}

void ensure_u32_len(std::size_t value, const char* name) {
    if (value > static_cast<std::size_t>(std::numeric_limits<uint32_t>::max())) {
        throw PortTunnelFrameError(std::string(name) + " length exceeds u32");
    }
}

}  // namespace

PortTunnelFrameError::PortTunnelFrameError(const std::string& message)
    : std::runtime_error(message) {}

const char* port_tunnel_preface() {
    return kPortTunnelPreface;
}

std::size_t port_tunnel_preface_size() {
    return sizeof(kPortTunnelPreface);
}

std::vector<unsigned char> encode_port_tunnel_frame(const PortTunnelFrame& frame) {
    if (frame.meta.size() > PORT_TUNNEL_MAX_META_LEN) {
        throw PortTunnelFrameError("port tunnel metadata exceeds maximum length");
    }
    if (frame.data.size() > PORT_TUNNEL_MAX_DATA_LEN) {
        throw PortTunnelFrameError("port tunnel data exceeds maximum length");
    }
    ensure_u32_len(frame.meta.size(), "metadata");
    ensure_u32_len(frame.data.size(), "data");

    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    bytes[0] = static_cast<unsigned char>(frame.type);
    bytes[1] = frame.flags;
    write_u32_be(bytes, 4U, frame.stream_id);
    write_u32_be(bytes, 8U, static_cast<uint32_t>(frame.meta.size()));
    write_u32_be(bytes, 12U, static_cast<uint32_t>(frame.data.size()));
    bytes.insert(bytes.end(), frame.meta.begin(), frame.meta.end());
    bytes.insert(bytes.end(), frame.data.begin(), frame.data.end());
    return bytes;
}

PortTunnelFrame decode_port_tunnel_frame(const std::vector<unsigned char>& bytes) {
    if (bytes.size() < PORT_TUNNEL_HEADER_LEN) {
        throw PortTunnelFrameError("port tunnel frame header is incomplete");
    }
    if (bytes[2] != 0U || bytes[3] != 0U) {
        throw PortTunnelFrameError("port tunnel reserved header bytes must be zero");
    }

    const uint32_t meta_len = read_u32_be(bytes, 8U);
    const uint32_t data_len = read_u32_be(bytes, 12U);
    if (meta_len > PORT_TUNNEL_MAX_META_LEN) {
        throw PortTunnelFrameError("port tunnel metadata exceeds maximum length");
    }
    if (data_len > PORT_TUNNEL_MAX_DATA_LEN) {
        throw PortTunnelFrameError("port tunnel data exceeds maximum length");
    }
    const std::size_t expected_size =
        PORT_TUNNEL_HEADER_LEN + static_cast<std::size_t>(meta_len) + static_cast<std::size_t>(data_len);
    if (bytes.size() != expected_size) {
        throw PortTunnelFrameError("port tunnel frame length mismatch");
    }

    PortTunnelFrame frame;
    frame.type = frame_type_from_byte(bytes[0]);
    frame.flags = bytes[1];
    frame.stream_id = read_u32_be(bytes, 4U);
    frame.meta.assign(
        reinterpret_cast<const char*>(&bytes[PORT_TUNNEL_HEADER_LEN]),
        static_cast<std::size_t>(meta_len)
    );
    const std::size_t data_offset = PORT_TUNNEL_HEADER_LEN + static_cast<std::size_t>(meta_len);
    frame.data.assign(bytes.begin() + data_offset, bytes.end());
    return frame;
}
