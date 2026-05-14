#include "test_server_streaming_shared.h"

static void assert_detached_session_expiry_does_not_consume_worker_budget(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 2UL);

    UniqueSocket listen_client;
    std::thread listen_thread;
    const PortTunnelFrame ready = open_v4_tunnel(state, &listen_client, &listen_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    close_tunnel(&listen_client, &listen_thread);

    UniqueSocket destination(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(destination.get());

    UniqueSocket connect_client_socket;
    std::thread connect_thread;
    open_v4_tunnel(state, &connect_client_socket, &connect_thread, "connect", "tcp", 1ULL);

    send_tunnel_frame(connect_client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}}));
    const PortTunnelFrame response = read_tunnel_frame(connect_client_socket.get());
    TEST_ASSERT(response.type == PortTunnelFrameType::TcpConnectOk);

    close_tunnel(&connect_client_socket, &connect_thread);

    UniqueSocket resumed_client;
    std::thread resumed_thread;
    open_v4_tunnel(state, &resumed_client, &resumed_thread, "listen", "tcp", 2ULL, session_id);
    close_tunnel(&resumed_client, &resumed_thread);
}

static void assert_tunnel_tcp_listener_session_can_resume_after_transport_drop(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready = open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL, session_id);

    UniqueSocket peer(connect_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(peer.valid());
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(accepted.type == PortTunnelFrameType::TcpAccept);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TunnelClose,
                   0U,
                   Json{{"forward_id", "fwd_cpp_test"}, {"generation", 1ULL}, {"reason", "operator_close"}}));
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

void assert_detached_session_releases_active_tcp_accept_budget(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready = open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket first_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame first_accept = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(first_accept.type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
    first_peer.reset();

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 2ULL, session_id);

    UniqueSocket second_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame second_accept = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(second_accept.type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_expired_tunnel_session_is_released(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready = open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();
    const unsigned long resume_timeout_ms = ready_meta.at("resume_timeout_ms").get<unsigned long>();

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);
    wait_past_resume_timeout(resume_timeout_ms);

    open_tunnel(state, &client_socket, &server_thread);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL, session_id)));
    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.type == PortTunnelFrameType::Error);
    const Json error_meta = Json::parse(error.meta);
    TEST_ASSERT(error_meta.at("code").get<std::string>() == "port_tunnel_resume_expired");

    close_tunnel(&client_socket, &server_thread);

    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(rebound.valid());
}

void assert_tunnel_resume_and_expiry_paths(AppState& state) {
    const fs::path root(state.config.default_workdir);

    assert_detached_session_expiry_does_not_consume_worker_budget(root);
    assert_tunnel_tcp_listener_session_can_resume_after_transport_drop(state);
    assert_expired_tunnel_session_is_released(state);
}
