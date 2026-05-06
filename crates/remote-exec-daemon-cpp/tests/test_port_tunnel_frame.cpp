#include "port_tunnel_frame.h"

#include <cassert>
#include <string>
#include <vector>

namespace {

void assert_decode_rejects(const std::vector<unsigned char>& bytes) {
    bool rejected = false;
    try {
        (void)decode_port_tunnel_frame(bytes);
    } catch (const PortTunnelFrameError&) {
        rejected = true;
    }
    assert(rejected);
}

std::vector<unsigned char> frame_header(
    unsigned char frame_type,
    uint32_t stream_id,
    uint32_t meta_len,
    uint32_t data_len
) {
    std::vector<unsigned char> bytes(16U, 0U);
    bytes[0] = frame_type;
    bytes[4] = static_cast<unsigned char>((stream_id >> 24) & 0xffU);
    bytes[5] = static_cast<unsigned char>((stream_id >> 16) & 0xffU);
    bytes[6] = static_cast<unsigned char>((stream_id >> 8) & 0xffU);
    bytes[7] = static_cast<unsigned char>(stream_id & 0xffU);
    bytes[8] = static_cast<unsigned char>((meta_len >> 24) & 0xffU);
    bytes[9] = static_cast<unsigned char>((meta_len >> 16) & 0xffU);
    bytes[10] = static_cast<unsigned char>((meta_len >> 8) & 0xffU);
    bytes[11] = static_cast<unsigned char>(meta_len & 0xffU);
    bytes[12] = static_cast<unsigned char>((data_len >> 24) & 0xffU);
    bytes[13] = static_cast<unsigned char>((data_len >> 16) & 0xffU);
    bytes[14] = static_cast<unsigned char>((data_len >> 8) & 0xffU);
    bytes[15] = static_cast<unsigned char>(data_len & 0xffU);
    return bytes;
}

}  // namespace

int main() {
    assert(std::string(port_tunnel_preface(), port_tunnel_preface_size()) == "REPFWD1\n");

    PortTunnelFrame frame;
    frame.type = PortTunnelFrameType::TcpData;
    frame.flags = 7U;
    frame.stream_id = 3U;
    frame.meta = "{\"note\":\"binary\"}";
    frame.data = {0U, 1U, 2U, 255U, static_cast<unsigned char>('R'), static_cast<unsigned char>('\n')};

    const std::vector<unsigned char> encoded = encode_port_tunnel_frame(frame);
    const PortTunnelFrame decoded = decode_port_tunnel_frame(encoded);
    assert(decoded.type == PortTunnelFrameType::TcpData);
    assert(decoded.flags == 7U);
    assert(decoded.stream_id == 3U);
    assert(decoded.meta == "{\"note\":\"binary\"}");
    assert(decoded.data == frame.data);

    assert_decode_rejects(frame_header(99U, 1U, 0U, 0U));
    assert_decode_rejects(frame_header(
        static_cast<unsigned char>(PortTunnelFrameType::Error),
        1U,
        static_cast<uint32_t>(PORT_TUNNEL_MAX_META_LEN + 1U),
        0U
    ));
    assert_decode_rejects(frame_header(
        static_cast<unsigned char>(PortTunnelFrameType::TcpData),
        1U,
        0U,
        static_cast<uint32_t>(PORT_TUNNEL_MAX_DATA_LEN + 1U)
    ));

    return 0;
}
