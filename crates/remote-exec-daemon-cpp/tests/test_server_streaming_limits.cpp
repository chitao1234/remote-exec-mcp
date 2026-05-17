#include "test_server_streaming_shared.h"

static const unsigned long TCP_PAYLOAD_DRAIN_MARGIN_MS = 100UL;

static void assert_tcp_connect_worker_limit_errors_before_success(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 1UL);

    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    UniqueSocket worker_holder;
    std::thread worker_holder_thread;
    open_v4_tunnel(state, &worker_holder, &worker_holder_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(worker_holder.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 99U, Json{{"endpoint", "127.0.0.1:0"}}));
    TEST_ASSERT(read_tunnel_frame(worker_holder.get()).type == PortTunnelFrameType::TcpListenOk);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}}));
    const PortTunnelFrame response = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(response.type == PortTunnelFrameType::Error);
    TEST_ASSERT(response.stream_id == 1U);
    const Json meta = Json::parse(response.meta);
    TEST_ASSERT(meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    TEST_ASSERT(meta.at("message").get<std::string>() == "port tunnel worker limit reached");
    TEST_ASSERT(!meta.at("fatal").get<bool>());

    close_tunnel(&worker_holder, &worker_holder_thread);
    close_tunnel(&client_socket, &server_thread);
}

static void assert_tcp_connect_read_thread_failure_errors_before_success(const fs::path& root) {
    AppState state;
    initialize_state(state, root);

    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    set_forced_tcp_read_thread_failures(1UL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}}));
    const PortTunnelFrame response = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(response.type == PortTunnelFrameType::Error);
    TEST_ASSERT(response.stream_id == 1U);
    const Json meta = Json::parse(response.meta);
    TEST_ASSERT(meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    TEST_ASSERT(meta.at("message").get<std::string>() == "port tunnel worker limit reached");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tcp_accept_read_thread_failure_drops_before_accept(const fs::path& root) {
    AppState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    set_forced_tcp_read_thread_failures(1UL);
    UniqueSocket dropped_peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame unexpected;
    TEST_ASSERT(!try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &unexpected));

    UniqueSocket accepted_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(accepted.type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_accept_read_thread_failure_drops_before_accept(const fs::path& root) {
    AppState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    set_forced_tcp_read_thread_failures(1UL);
    UniqueSocket dropped_peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame unexpected;
    TEST_ASSERT(!try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &unexpected));

    UniqueSocket accepted_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(accepted.type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_accept_worker_pressure_is_local_drop(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 1UL);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame drop_report;
    TEST_ASSERT(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "tcp_stream", "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_udp_bind_worker_failure_releases_udp_budget(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_worker_threads = 0UL;
    limits.max_udp_binds = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);

    for (int attempt = 0; attempt < 2; ++attempt) {
        send_tunnel_frame(client_socket.get(),
                          json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
        const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
        TEST_ASSERT(error.type == PortTunnelFrameType::Error);
        const Json meta = Json::parse(error.meta);
        TEST_ASSERT(meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
        TEST_ASSERT(meta.at("message").get<std::string>() == "port tunnel worker limit reached");
    }

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_accept_pressure_is_local_drop(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_worker_threads = 3UL;
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string hold_endpoint = socket_local_endpoint(listener.get());

    UniqueSocket connect_client;
    std::thread connect_thread;
    open_v4_tunnel(state, &connect_client, &connect_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(connect_client.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 3U, Json{{"endpoint", hold_endpoint}}));
    TEST_ASSERT(read_tunnel_frame(connect_client.get()).type == PortTunnelFrameType::TcpConnectOk);
    UniqueSocket held_peer(accept(listener.get(), NULL, NULL));
    TEST_ASSERT(held_peer.valid());

    UniqueSocket refused_peer(connect_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(refused_peer.valid());
    PortTunnelFrame drop_report;
    TEST_ASSERT(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "tcp_stream", "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
    close_tunnel(&connect_client, &connect_thread);
}

static std::thread accept_and_send_tcp_payload(SOCKET listener_socket, const std::vector<unsigned char>& payload) {
    return std::thread([listener_socket, payload]() {
        UniqueSocket accepted(accept(listener_socket, NULL, NULL));
        TEST_ASSERT(accepted.valid());
        send_all_bytes(accepted.get(), reinterpret_cast<const char*>(payload.data()), payload.size());
        platform::sleep_ms(TCP_PAYLOAD_DRAIN_MARGIN_MS);
    });
}

static std::string closed_loopback_tcp_endpoint() {
    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());
    listener.reset();
    return endpoint;
}

static void assert_active_tcp_stream_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket echo_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(echo_listener.get());

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 7U, Json{{"endpoint", closed_loopback_tcp_endpoint()}}));
    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "port_connect_failed");

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);
    UniqueSocket first_accepted(accept(echo_listener.get(), NULL, NULL));
    TEST_ASSERT(first_accepted.valid());

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 3U, Json{{"endpoint", endpoint}}));
    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "port_tunnel_limit_exceeded");
    TEST_ASSERT(!tcp_listener_has_pending_connection(echo_listener.get(), 100UL));

    send_tunnel_frame(client_socket.get(), empty_frame(PortTunnelFrameType::Close, 1U));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    first_accepted.reset();

    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 5U, Json{{"endpoint", endpoint}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);
    UniqueSocket second_accepted(accept(echo_listener.get(), NULL, NULL));
    TEST_ASSERT(second_accepted.valid());

    close_tunnel(&client_socket, &server_thread);
}

static void assert_active_tcp_accept_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket first_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame first_accept = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(first_accept.type == PortTunnelFrameType::TcpAccept);

    UniqueSocket refused_peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame drop_report;
    TEST_ASSERT(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "tcp_stream", "port_tunnel_limit_exceeded");
    refused_peer.reset();

    send_tunnel_frame(client_socket.get(), empty_frame(PortTunnelFrameType::Close, first_accept.stream_id));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    first_peer.reset();

    UniqueSocket second_peer(connect_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_queued_byte_limit_is_enforced(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_tunnel_queued_bytes = 128UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket payload_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(payload_listener.get());
    std::vector<unsigned char> payload(512U, 42U);
    std::thread sender_thread = accept_and_send_tcp_payload(payload_listener.get(), payload);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);

    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
    payload_listener.reset();
    sender_thread.join();
}

static void assert_udp_queued_byte_pressure_reports_drop(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_tunnel_queued_bytes = 128UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const std::string endpoint = Json::parse(bind_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer(bind_port_forward_socket("127.0.0.1:0", "udp"));
    socklen_t peer_len = 0;
    const sockaddr_storage destination = parse_port_forward_peer(endpoint, &peer_len);
    std::vector<unsigned char> payload(512U, 7U);
    TEST_ASSERT(sendto(peer.get(),
                       reinterpret_cast<const char*>(payload.data()),
                       static_cast<int>(payload.size()),
                       0,
                       reinterpret_cast<const sockaddr*>(&destination),
                       peer_len) == static_cast<int>(payload.size()));

    PortTunnelFrame drop_report;
    TEST_ASSERT(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "udp_datagram", "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_partial_tunnel_frame_times_out(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.tunnel_io_timeout_ms = 50UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    const unsigned char partial_header[2] = {
        static_cast<unsigned char>(PortTunnelFrameType::TcpData),
        0U,
    };
    send_all_bytes(client_socket.get(), reinterpret_cast<const char*>(partial_header), sizeof(partial_header));

    server_thread.join();
    client_socket.reset();
}

static void assert_tcp_data_write_pressure_does_not_block_control_frames(AppState& state) {
    UniqueSocket hold_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string hold_endpoint = socket_local_endpoint(hold_listener.get());
    std::atomic<bool> receiver_ready(false);
    std::atomic<bool> release_receiver(false);
    std::thread hold_thread([&]() {
        UniqueSocket accepted(accept(hold_listener.get(), NULL, NULL));
        TEST_ASSERT(accepted.valid());
        int buffer_size = 1024;
        setsockopt(
            accepted.get(), SOL_SOCKET, SO_RCVBUF, reinterpret_cast<const char*>(&buffer_size), sizeof(buffer_size));
        receiver_ready.store(true);
        while (!release_receiver.load()) {
            platform::sleep_ms(10UL);
        }
    });

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(client_socket.get(),
                      json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", hold_endpoint}}));
    TEST_ASSERT(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);
    TEST_ASSERT(wait_until_true(receiver_ready, 1000UL));

    std::vector<unsigned char> payload(PORT_TUNNEL_MAX_DATA_LEN, 0x51U);
    PortTunnelFrame heartbeat = empty_frame(PortTunnelFrameType::TunnelHeartbeat, 0U);
    heartbeat.meta = Json{{"nonce", 1}}.dump();
    std::thread writer_thread([&]() {
        for (int i = 0; i < 64; ++i) {
            send_tunnel_frame(client_socket.get(), data_frame(PortTunnelFrameType::TcpData, 1U, payload));
        }
        send_tunnel_frame(client_socket.get(), heartbeat);
    });

    const std::uint64_t deadline = platform::monotonic_ms() + 2000ULL;
    bool saw_ack = false;
    while (platform::monotonic_ms() < deadline) {
        PortTunnelFrame frame;
        if (!try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &frame)) {
            continue;
        }
        if (frame.type == PortTunnelFrameType::TunnelHeartbeatAck) {
            TEST_ASSERT(frame.meta == heartbeat.meta);
            saw_ack = true;
            break;
        }
    }
    TEST_ASSERT(saw_ack);

    writer_thread.join();
    release_receiver.store(true);
    hold_thread.join();
    close_tunnel(&client_socket, &server_thread);
    hold_listener.reset();
}

void assert_tunnel_limit_and_pressure_paths(AppState& state) {
    const fs::path root(state.config.default_workdir);

    assert_tcp_connect_worker_limit_errors_before_success(root);
    assert_tcp_connect_read_thread_failure_errors_before_success(root);
    assert_tcp_accept_read_thread_failure_drops_before_accept(root);
    assert_retained_tcp_accept_read_thread_failure_drops_before_accept(root);
    assert_retained_tcp_accept_worker_pressure_is_local_drop(root);
    assert_retained_udp_bind_worker_failure_releases_udp_budget(root);
    assert_retained_tcp_accept_pressure_is_local_drop(root);
    assert_active_tcp_stream_limit_is_enforced_and_released(root);
    assert_active_tcp_accept_limit_is_enforced_and_released(root);
    assert_tunnel_queued_byte_limit_is_enforced(root);
    assert_udp_queued_byte_pressure_reports_drop(root);
    assert_partial_tunnel_frame_times_out(root);
    assert_tcp_data_write_pressure_does_not_block_control_frames(state);
}
