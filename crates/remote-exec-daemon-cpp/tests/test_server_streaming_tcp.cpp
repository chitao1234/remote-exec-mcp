#include "test_server_streaming_shared.h"

static void assert_tunnel_close_releases_tcp_listener(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TunnelClose,
                   0U,
                   Json{{"forward_id", "fwd_cpp_test"}, {"generation", 1ULL}, {"reason", "operator_close"}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelClosed);
    close_tunnel(&client_socket, &server_thread);

    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(rebound.valid());
}

static void assert_terminal_tunnel_error_releases_tcp_listener_immediately(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    const unsigned char invalid_frame[PORT_TUNNEL_HEADER_LEN] = {
        static_cast<unsigned char>(PortTunnelFrameType::TcpData),
        0U,
        1U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
    };
    send_all_bytes(client_socket.get(), reinterpret_cast<const char*>(invalid_frame), sizeof(invalid_frame));

    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.type == PortTunnelFrameType::Error);
    TEST_ASSERT(error.stream_id == 0U);
    const Json error_meta = Json::parse(error.meta);
    TEST_ASSERT(error_meta.at("code").get<std::string>() == "invalid_port_tunnel");
    TEST_ASSERT(error_meta.at("fatal").get<bool>());

    close_tunnel(&client_socket, &server_thread);
    wait_until_bindable(endpoint);
}

static void assert_tunnel_close_releases_retained_listener_immediately(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelReady);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TunnelClose,
                   0U,
                   Json{{"forward_id", "fwd_cpp_test"}, {"generation", 1ULL}, {"reason", "operator_close"}}));
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(closed.stream_id == 0U);

    close_tunnel(&client_socket, &server_thread);
    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(rebound.valid());
}

static void assert_tunnel_tcp_connect_echoes_binary_data(AppState& state) {
    UniqueSocket echo_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string echo_endpoint = socket_local_endpoint(echo_listener.get());
    std::thread echo_thread([&]() {
        UniqueSocket accepted(accept(echo_listener.get(), NULL, NULL));
        TEST_ASSERT(accepted.valid());
        char buffer[64];
        const int received = recv(accepted.get(), buffer, sizeof(buffer), 0);
        TEST_ASSERT(received > 0);
        send_all_bytes(accepted.get(), buffer, static_cast<std::size_t>(received));
    });

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", echo_endpoint}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);
    const std::vector<unsigned char> payload = {
        0U, 1U, 2U, 255U, static_cast<unsigned char>('x'), static_cast<unsigned char>('\n')};
    send_tunnel_frame(client_socket.get(), data_frame(PortTunnelFrameType::TcpData, 1U, payload));
    const PortTunnelFrame echoed = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(echoed.type == PortTunnelFrameType::TcpData);
    TEST_ASSERT(echoed.data == payload);

    close_tunnel(&client_socket, &server_thread);
    echo_listener.reset();
    echo_thread.join();
}

void assert_tunnel_tcp_listener_and_connect_paths(AppState& state) {
    assert_tunnel_close_releases_tcp_listener(state);
    assert_terminal_tunnel_error_releases_tcp_listener_immediately(state);
    assert_tunnel_close_releases_retained_listener_immediately(state);
    assert_tunnel_tcp_connect_echoes_binary_data(state);
}
