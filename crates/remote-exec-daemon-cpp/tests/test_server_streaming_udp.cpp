#include "test_server_streaming_shared.h"

#include <algorithm>

static void assert_udp_bind_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_udp_binds = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "udp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::UdpBindOk);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 3U, Json{{"endpoint", "127.0.0.1:0"}}));
    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "port_tunnel_limit_exceeded");

    send_tunnel_frame(client_socket.get(), empty_frame(PortTunnelFrameType::Close, 1U));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 5U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::UdpBindOk);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_udp_bind_emits_two_peer_datagrams(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const std::string endpoint = Json::parse(bind_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer_a(bind_port_forward_socket("127.0.0.1:0", "udp"));
    UniqueSocket peer_b(bind_port_forward_socket("127.0.0.1:0", "udp"));
    socklen_t peer_len = 0;
    const sockaddr_storage peer = parse_port_forward_peer(endpoint, &peer_len);
    TEST_ASSERT(sendto(peer_a.get(), "udp-a", 5, 0, reinterpret_cast<const sockaddr*>(&peer), peer_len) == 5);
    TEST_ASSERT(sendto(peer_b.get(), "udp-b", 5, 0, reinterpret_cast<const sockaddr*>(&peer), peer_len) == 5);

    const PortTunnelFrame first = read_tunnel_frame(client_socket.get());
    const PortTunnelFrame second = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(first.type == PortTunnelFrameType::UdpDatagram);
    TEST_ASSERT(second.type == PortTunnelFrameType::UdpDatagram);
    std::vector<std::string> payloads;
    payloads.push_back(std::string(first.data.begin(), first.data.end()));
    payloads.push_back(std::string(second.data.begin(), second.data.end()));
    std::sort(payloads.begin(), payloads.end());
    TEST_ASSERT(payloads[0] == "udp-a");
    TEST_ASSERT(payloads[1] == "udp-b");

    close_tunnel(&client_socket, &server_thread);
}

void assert_tunnel_udp_paths(AppState& state) {
    const fs::path root(state.config.default_workdir);

    assert_udp_bind_limit_is_enforced_and_released(root);
    assert_tunnel_udp_bind_emits_two_peer_datagrams(state);
}
