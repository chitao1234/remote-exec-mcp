#include "test_server_streaming_shared.h"

#include "../src/port_forward/port_tunnel_connection.h"
#include "../src/port_forward/port_tunnel_service.h"
#include "test_socket_pair.h"

#include "platform/deadline.h"
#include "port_forward/port_forward_error.h"

#include <cstdio>
#include <cstdlib>
#include <exception>
#include <memory>

static PortTunnelFrame read_required_tunnel_frame_with_timeout(
    SOCKET socket,
    unsigned long timeout_ms
) {
    PortTunnelFrame frame;
    TEST_ASSERT(try_read_tunnel_frame_with_timeout(socket, timeout_ms, &frame));
    return frame;
}

static std::shared_ptr<PortTunnelConnection> make_test_tunnel_connection(
    const std::shared_ptr<PortTunnelService>& service
) {
    return std::shared_ptr<PortTunnelConnection>(new PortTunnelConnection(INVALID_SOCKET, service));
}

static void assert_session_transition_rules_are_strict() {
    std::shared_ptr<PortTunnelService> service =
        create_port_tunnel_service(PortForwardLimitConfig());
    std::shared_ptr<PortTunnelSession> session = service->create_session();
    std::shared_ptr<PortTunnelConnection> first_connection = make_test_tunnel_connection(service);
    std::shared_ptr<PortTunnelConnection> second_connection = make_test_tunnel_connection(service);

    bool detached = true;
    TEST_ASSERT(
        session->detach_until(platform::monotonic_deadline_after_ms(1000UL), &detached).get()
        == nullptr
    );
    TEST_ASSERT(!detached);
    TEST_ASSERT(session->state == PortTunnelSessionState::New);

    TEST_ASSERT(
        session->attach_resumed(first_connection, 1ULL, platform::monotonic_ms())
        == PortTunnelSessionResumeResult::Unknown
    );
    TEST_ASSERT(session->state == PortTunnelSessionState::New);

    TEST_ASSERT(session->attach_new(first_connection, 1ULL));
    TEST_ASSERT(session->state == PortTunnelSessionState::Attached);
    TEST_ASSERT(session->generation == 1ULL);
    TEST_ASSERT(session->current_attachment().get() != nullptr);

    TEST_ASSERT(!session->attach_new(second_connection, 2ULL));
    TEST_ASSERT(
        session->attach_resumed(second_connection, 2ULL, platform::monotonic_ms())
        == PortTunnelSessionResumeResult::AlreadyAttached
    );
    TEST_ASSERT(session->state == PortTunnelSessionState::Attached);
    TEST_ASSERT(session->generation == 1ULL);

    std::shared_ptr<PortTunnelSessionAttachment> first_attachment =
        session->detach_until(platform::monotonic_deadline_after_ms(1000UL), &detached);
    TEST_ASSERT(detached);
    TEST_ASSERT(first_attachment.get() != nullptr);
    TEST_ASSERT(session->state == PortTunnelSessionState::Detached);
    TEST_ASSERT(session->current_attachment().get() == nullptr);

    TEST_ASSERT(
        session->detach_until(platform::monotonic_deadline_after_ms(1000UL), &detached).get()
        == nullptr
    );
    TEST_ASSERT(!detached);
    TEST_ASSERT(!session->attach_new(first_connection, 3ULL));
    TEST_ASSERT(session->state == PortTunnelSessionState::Detached);

    TEST_ASSERT(
        session->attach_resumed(second_connection, 2ULL, platform::monotonic_ms())
        == PortTunnelSessionResumeResult::Ready
    );
    TEST_ASSERT(session->state == PortTunnelSessionState::Attached);
    TEST_ASSERT(session->generation == 2ULL);

    PortTunnelSessionTeardown teardown = session->close_terminal(false);
    TEST_ASSERT(teardown.transitioned);
    TEST_ASSERT(session->state == PortTunnelSessionState::Closed);
    TEST_ASSERT(
        session->attach_resumed(first_connection, 3ULL, platform::monotonic_ms())
        == PortTunnelSessionResumeResult::Unknown
    );

    service->shutdown();
}

static void assert_attached_session_rejects_second_resume(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const std::string session_id = Json::parse(ready.meta).at("session_id").get<std::string>();

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 2ULL, session_id);

    UniqueSocket second_client;
    std::thread second_thread;
    open_tunnel(state, &second_client, &second_thread);
    send_tunnel_frame(
        second_client.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 3ULL, session_id)
        )
    );
    assert_tunnel_error_code(
        read_required_tunnel_frame_with_timeout(second_client.get(), 1000UL),
        "port_tunnel_already_attached"
    );

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 2ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );
    TEST_ASSERT(
        read_required_tunnel_frame_with_timeout(client_socket.get(), 1000UL).type
        == PortTunnelFrameType::TunnelClosed
    );

    close_tunnel(&second_client, &second_thread);
    close_tunnel(&client_socket, &server_thread);
}

static void assert_detached_session_expiry_does_not_consume_worker_budget(const fs::path& root) {
    TestDaemonState state;
    initialize_state_with_worker_limit(state, root, 2UL);

    UniqueSocket listen_client;
    std::thread listen_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &listen_client, &listen_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    close_tunnel(&listen_client, &listen_thread);

    UniqueSocket destination(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(destination.get());

    UniqueSocket connect_client_socket;
    std::thread connect_thread;
    open_v4_tunnel(state, &connect_client_socket, &connect_thread, "connect", "tcp", 1ULL);

    send_tunnel_frame(
        connect_client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}})
    );
    const PortTunnelFrame response = read_tunnel_frame(connect_client_socket.get());
    TEST_ASSERT(response.type == PortTunnelFrameType::TcpConnectOk);

    close_tunnel(&connect_client_socket, &connect_thread);

    UniqueSocket resumed_client;
    std::thread resumed_thread;
    open_v4_tunnel(state, &resumed_client, &resumed_thread, "listen", "tcp", 2ULL, session_id);
    close_tunnel(&resumed_client, &resumed_thread);
}

static void assert_tunnel_tcp_listener_session_can_resume_after_transport_drop(
    TestDaemonState& state
) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
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
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_udp_bind_session_can_resume_after_transport_drop(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const std::string endpoint = Json::parse(bind_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 2ULL, session_id);

    UniqueSocket peer(bind_port_forward_socket("127.0.0.1:0", "udp"));
    const SocketAddress destination = parse_port_forward_peer(endpoint);

    PortTunnelFrame datagram;
    bool received = false;
    for (int attempt = 0; attempt < 20 && !received; ++attempt) {
        TEST_ASSERT(
            sendto(
                peer.get(),
                "resume-udp",
                10,
                0,
                destination.sockaddr_ptr(),
                destination.address_len
            )
            == 10
        );
        received = try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &datagram);
    }
    TEST_ASSERT(received);
    TEST_ASSERT(datagram.type == PortTunnelFrameType::UdpDatagram);
    TEST_ASSERT(std::string(datagram.data.begin(), datagram.data.end()) == "resume-udp");

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 2ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 2ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_resumed_session_rejects_stale_generation_close(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 2ULL, session_id);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );
    PortTunnelFrame error = read_required_tunnel_frame_with_timeout(client_socket.get(), 1000UL);
    assert_tunnel_error_code(error, "port_tunnel_generation_mismatch");

    UniqueSocket peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame accepted =
        read_required_tunnel_frame_with_timeout(client_socket.get(), 1000UL);
    TEST_ASSERT(accepted.type == PortTunnelFrameType::TcpAccept);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 2ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );
    const PortTunnelFrame closed =
        read_required_tunnel_frame_with_timeout(client_socket.get(), 1000UL);
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 2ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_listener_closes_while_accept_worker_waits(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);

    platform::sleep_ms(25UL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );

    const PortTunnelFrame closed =
        read_required_tunnel_frame_with_timeout(client_socket.get(), 1000UL);
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_udp_bind_closes_while_read_worker_waits(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(bind_ok.type == PortTunnelFrameType::UdpBindOk);

    platform::sleep_ms(25UL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", kTunnelCloseReasonOperatorClose}
            }
        )
    );

    const PortTunnelFrame closed =
        read_required_tunnel_frame_with_timeout(client_socket.get(), 1000UL);
    TEST_ASSERT(closed.type == PortTunnelFrameType::TunnelClosed);
    TEST_ASSERT(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

void assert_detached_session_releases_active_tcp_accept_budget(const fs::path& root) {
    PortForwardLimitConfig limits;
    limits.max_active_tcp_streams = 1UL;

    TestDaemonState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
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

static void assert_service_shutdown_releases_detached_retained_listener(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);

    state.services.port_tunnel->shutdown();
    state.services.port_tunnel->shutdown();
    state.services.port_tunnel.reset();
    wait_until_bindable(endpoint);
}

static void assert_service_shutdown_closes_retained_listener_with_active_stream(const fs::path& root
) {
    TestDaemonState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(accepted.type == PortTunnelFrameType::TcpAccept);

    state.services.port_tunnel->shutdown();
    state.services.port_tunnel->shutdown();
    wait_until_bindable(endpoint);

    TEST_ASSERT(socket_readable_within(peer.get(), 1000UL));
    char buffer = '\0';
    const int received = recv(peer.get(), &buffer, 1, 0);
    TEST_ASSERT(received <= 0);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_service_shutdown_rejects_new_sessions() {
    std::shared_ptr<PortTunnelService> service =
        create_port_tunnel_service(PortForwardLimitConfig());
    service->shutdown();
    service->shutdown();

    bool rejected = false;
    try {
        service->create_session();
    } catch (const PortForwardError& ex) {
        rejected = ex.code() == "port_tunnel_shutting_down";
    }
    TEST_ASSERT(rejected);
}

static void assert_resource_close_paths_are_idempotent() {
    std::shared_ptr<RetainedTcpListener> listener(new RetainedTcpListener(
        1U,
        bind_port_forward_socket("127.0.0.1:0", "tcp"),
        PortTunnelBudgetLease()
    ));
    TEST_ASSERT(listener->resource_state_snapshot() == PortTunnelResourceState::Open);
    listener->close();
    listener->close();
    TEST_ASSERT(listener->is_closed());
    TEST_ASSERT(listener->resource_state_snapshot() == PortTunnelResourceState::Closed);

    std::shared_ptr<TunnelUdpSocket> udp_bind(
        new TunnelUdpSocket(bind_port_forward_socket("127.0.0.1:0", "udp"), PortTunnelBudgetLease())
    );
    TEST_ASSERT(udp_bind->resource_state_snapshot() == PortTunnelResourceState::Open);
    udp_bind->close();
    udp_bind->close();
    TEST_ASSERT(udp_bind->is_closed());
    TEST_ASSERT(udp_bind->resource_state_snapshot() == PortTunnelResourceState::Closed);

    ConnectedSocketPair sockets = make_connected_socket_pair();
    std::shared_ptr<TunnelTcpStream> stream(
        new TunnelTcpStream(sockets.first.release(), PortTunnelBudgetLease())
    );
    TEST_ASSERT(stream->resource_state_snapshot() == PortTunnelResourceState::Open);
    stream->close();
    stream->close();
    TEST_ASSERT(stream->is_closed());
    TEST_ASSERT(stream->resource_state_snapshot() == PortTunnelResourceState::Closed);
}

static void assert_sender_close_handles_queued_control_frames() {
    PortForwardLimitConfig limits;
    std::shared_ptr<PortTunnelService> service = create_port_tunnel_service(limits);
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket peer_socket(std::move(sockets.second));
    std::shared_ptr<PortTunnelConnection> connection(
        new PortTunnelConnection(server_socket.get(), service)
    );

    for (int i = 0; i < 4096; ++i) {
        connection->send_error(1U, "queued_close_test", "queued close test frame");
    }

    peer_socket.reset();
    connection.reset();
    server_socket.reset();
    service->shutdown();
}

static void assert_expired_tunnel_session_is_released(TestDaemonState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();
    const unsigned long resume_timeout_ms = ready_meta.at("resume_timeout_ms").get<unsigned long>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);
    wait_past_resume_timeout(resume_timeout_ms);
    const std::uint64_t removal_deadline = platform::monotonic_ms() + 2000ULL;
    while (state.services.port_tunnel->find_session(session_id).get() != nullptr
           && platform::monotonic_ms() < removal_deadline) {
        platform::sleep_ms(10UL);
    }
    TEST_ASSERT(state.services.port_tunnel->find_session(session_id).get() == nullptr);

    open_tunnel(state, &client_socket, &server_thread);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL, session_id)
        )
    );
    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(error.type == PortTunnelFrameType::Error);
    const Json error_meta = Json::parse(error.meta);
    TEST_ASSERT(error_meta.at("code").get<std::string>() == "unknown_port_tunnel_session");

    close_tunnel(&client_socket, &server_thread);

    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    TEST_ASSERT(rebound.valid());
}

static void assert_detached_listener_expiry_survives_last_external_service_ref(const fs::path& root
) {
    TestDaemonState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const unsigned long resume_timeout_ms = ready_meta.at("resume_timeout_ms").get<unsigned long>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);
    state.services.port_tunnel.reset();
    wait_past_resume_timeout(resume_timeout_ms);
    wait_until_bindable(endpoint);
}

static void wait_until_udp_bindable(const std::string& endpoint) {
    for (int attempt = 0; attempt < 40; ++attempt) {
        try {
            UniqueSocket rebound(bind_port_forward_socket(endpoint, "udp"));
            if (rebound.valid()) {
                return;
            }
        } catch (const std::exception&) {
        }
        platform::sleep_ms(25UL);
    }
    std::fprintf(stderr, "udp endpoint `%s` did not become bindable\n", endpoint.c_str());
    std::abort();
}

static void assert_detached_udp_bind_expiry_survives_last_external_service_ref(const fs::path& root
) {
    TestDaemonState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const unsigned long resume_timeout_ms = ready_meta.at("resume_timeout_ms").get<unsigned long>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    TEST_ASSERT(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const std::string endpoint = Json::parse(bind_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);
    state.services.port_tunnel.reset();
    wait_past_resume_timeout(resume_timeout_ms);
    wait_until_udp_bindable(endpoint);
}

void assert_tunnel_resume_and_expiry_paths(TestDaemonState& state) {
    const fs::path root(state.config.default_workdir);

    assert_session_transition_rules_are_strict();
    assert_attached_session_rejects_second_resume(state);
    assert_detached_session_expiry_does_not_consume_worker_budget(root);
    assert_tunnel_tcp_listener_session_can_resume_after_transport_drop(state);
    assert_tunnel_udp_bind_session_can_resume_after_transport_drop(state);
    assert_resumed_session_rejects_stale_generation_close(state);
    assert_retained_tcp_listener_closes_while_accept_worker_waits(state);
    assert_retained_udp_bind_closes_while_read_worker_waits(state);
    assert_service_shutdown_releases_detached_retained_listener(state);
    assert_service_shutdown_closes_retained_listener_with_active_stream(root);
    assert_service_shutdown_rejects_new_sessions();
    assert_resource_close_paths_are_idempotent();
    assert_sender_close_handles_queued_control_frames();
    assert_expired_tunnel_session_is_released(state);
    assert_detached_listener_expiry_survives_last_external_service_ref(root);
    assert_detached_udp_bind_expiry_survives_last_external_service_ref(root);
}
