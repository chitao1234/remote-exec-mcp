#include "test_server_streaming_shared.h"

static void assert_tunnel_open_ready_and_close_round_trip(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    const PortTunnelFrame ready = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);
    const Json ready_meta = Json::parse(ready.meta);
    TEST_ASSERT(ready_meta.at("generation").get<uint64_t>() == 1ULL);
    TEST_ASSERT(ready_meta.at("session_id").get<std::string>().find("ptun_") == 0);
    TEST_ASSERT(ready_meta.at("resume_timeout_ms").get<unsigned long>() > 0UL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TunnelClose,
                   0U,
                   Json{{"forward_id", "fwd_cpp_test"}, {"generation", 1ULL}, {"reason", "operator_close"}}));
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(closed.stream_id == 0U);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_port_tunnel_worker_limit_is_reported(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 1UL);

    UniqueSocket worker_holder;
    std::thread worker_holder_thread;
    open_tunnel(state, &worker_holder, &worker_holder_thread);

    send_tunnel_frame(worker_holder.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    TEST_ASSERT(read_tunnel_frame(worker_holder.get()).type == PortTunnelFrameType::TunnelReady);

    send_tunnel_frame(worker_holder.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(worker_holder.get()).type == PortTunnelFrameType::TcpListenOk);

    UniqueSocket limited_client;
    std::thread limited_thread;
    open_tunnel(state, &limited_client, &limited_thread);
    send_tunnel_frame(limited_client.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    TEST_ASSERT(read_tunnel_frame(limited_client.get()).type == PortTunnelFrameType::TunnelReady);

    send_tunnel_frame(limited_client.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame error = read_tunnel_frame(limited_client.get());
    TEST_ASSERT(error.type == PortTunnelFrameType::Error);
    TEST_ASSERT(error.stream_id == 1U);
    const Json error_meta = Json::parse(error.meta);
    TEST_ASSERT(error_meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    TEST_ASSERT(error_meta.at("message").get<std::string>() == "port tunnel worker limit reached");
    TEST_ASSERT(!error_meta.at("fatal").get<bool>());

    close_tunnel(&limited_client, &limited_thread);
    close_tunnel(&worker_holder, &worker_holder_thread);
}

static void assert_tunnel_ready_reports_configured_limits(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_worker_threads = 7UL;
    limits.max_retained_sessions = 2UL;
    limits.max_retained_listeners = 4UL;
    limits.max_udp_binds = 5UL;
    limits.max_active_tcp_streams = 3UL;
    limits.max_tunnel_queued_bytes = 4096UL;
    limits.tunnel_io_timeout_ms = 6000UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    const PortTunnelFrame ready = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);
    const Json ready_meta = Json::parse(ready.meta);
    const Json ready_limits = ready_meta.at("limits");
    TEST_ASSERT(ready_limits.at("max_active_tcp_streams").get<unsigned long>() == 3UL);
    TEST_ASSERT(ready_limits.at("max_udp_peers").get<unsigned long>() == 5UL);
    TEST_ASSERT(ready_limits.at("max_queued_bytes").get<unsigned long>() == 4096UL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_rejects_data_plane_before_open(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", "127.0.0.1:1"}}));
    PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 1U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 3U, Json{{"endpoint", "127.0.0.1:0"}}));
    error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 3U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_open_metadata_error(AppState& state, const std::string& meta) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    PortTunnelFrame frame = empty_frame(PortTunnelFrameType::TunnelOpen, 0U);
    frame.meta = meta;
    send_tunnel_frame(client_socket.get(), frame);

    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 0U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_rejects_frames_for_wrong_role_or_protocol(AppState& state) {
    assert_tunnel_open_metadata_error(state, "{not-json");
    assert_tunnel_open_metadata_error(state, Json{{"role", "listen"}, {"protocol", "tcp"}}.dump());
    assert_tunnel_open_metadata_error(state, Json{{"role", 7}, {"protocol", "tcp"}, {"generation", 1ULL}}.dump());
    assert_tunnel_open_metadata_error(state,
                                      Json{{"role", "listen"}, {"protocol", "tcp"}, {"generation", "bad"}}.dump());

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 1U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 3U, Json{{"endpoint", "127.0.0.1:0"}}));
    error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 3U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("connect", "tcp", 2ULL)));
    error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 0U);
    assert_tunnel_error_code(error, "port_tunnel_already_attached");

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 5U, Json{{"endpoint", "127.0.0.1:0"}}));
    error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.stream_id == 5U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_legacy_session_frames_are_reserved_but_unsupported(AppState& state) {
    const PortTunnelFrameType legacy_frames[] = {PortTunnelFrameType::SessionOpen, PortTunnelFrameType::SessionResume};
    for (std::size_t i = 0U; i < sizeof(legacy_frames) / sizeof(legacy_frames[0]); ++i) {
        UniqueSocket client_socket;
        std::thread server_thread;
        open_tunnel(state, &client_socket, &server_thread);

        send_tunnel_frame(client_socket.get(),
                          json_frame(legacy_frames[i], 0U, Json{{"session_id", "legacy_session"}}));
        assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "invalid_port_tunnel");

        close_tunnel(&client_socket, &server_thread);
    }
}

static void assert_retained_session_limit_is_enforced(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_retained_sessions = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket first_client;
    std::thread first_thread;
    open_v4_tunnel(state, &first_client, &first_thread, "listen", "tcp", 1ULL);

    UniqueSocket second_client;
    std::thread second_thread;
    open_tunnel(state, &second_client, &second_thread);
    send_tunnel_frame(second_client.get(),
                      json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    assert_tunnel_error_code(read_tunnel_frame(second_client.get()), "port_tunnel_limit_exceeded");

    close_tunnel(&second_client, &second_thread);
    close_tunnel(&first_client, &first_thread);
}

static void assert_retained_listener_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_retained_sessions = 2UL;
    limits.max_retained_listeners = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket first_client;
    std::thread first_thread;
    open_v4_tunnel(state, &first_client, &first_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(first_client.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(first_client.get()).type == PortTunnelFrameType::TcpListenOk);

    UniqueSocket second_client;
    std::thread second_thread;
    open_v4_tunnel(state, &second_client, &second_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(second_client.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    assert_tunnel_error_code(read_tunnel_frame(second_client.get()), "port_tunnel_limit_exceeded");

    send_tunnel_frame(first_client.get(), empty_frame(PortTunnelFrameType::Close, 1U));
    TEST_ASSERT(read_tunnel_frame(first_client.get()).type == PortTunnelFrameType::Close);
    close_tunnel(&first_client, &first_thread);

    open_v4_tunnel(state, &first_client, &first_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(first_client.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(first_client.get()).type == PortTunnelFrameType::TcpListenOk);

    close_tunnel(&second_client, &second_thread);
    close_tunnel(&first_client, &first_thread);
}

void assert_listen_session_rejects_second_retained_open(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpListenOk);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 3U, Json{{"endpoint", "127.0.0.1:0"}}));
    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "invalid_port_tunnel");
    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::UdpBindOk);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 3U, Json{{"endpoint", "127.0.0.1:0"}}));
    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "invalid_port_tunnel");
    close_tunnel(&client_socket, &server_thread);
}

void assert_tunnel_rejects_invalid_requests(AppState& state) {
    assert_tunnel_rejects_data_plane_before_open(state);
    assert_tunnel_rejects_frames_for_wrong_role_or_protocol(state);
    assert_legacy_session_frames_are_reserved_but_unsupported(state);
}

void assert_tunnel_open_ready_and_limits(AppState& state) {
    const fs::path root(state.config.default_workdir);

    assert_tunnel_open_ready_and_close_round_trip(state);
    assert_tunnel_ready_reports_configured_limits(root);
    assert_port_tunnel_worker_limit_is_reported(root);
    assert_retained_session_limit_is_enforced(root);
    assert_retained_listener_limit_is_enforced_and_released(root);
}
