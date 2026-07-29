#pragma once

#include <atomic>
#include <cstdint>
#include <string>
#include <thread>
#include <vector>

#include "core/config.h"
#include "http/http_helpers.h"
#include "http/server_transport.h"
#include "platform/platform.h"
#include "port_forward/port_forward_endpoint.h"
#include "port_forward/port_forward_socket_ops.h"
#include "port_forward/port_tunnel.h"
#include "port_forward/port_tunnel_frame.h"
#include "runtime/app_context.h"
#include "test_assert.h"
#include "test_daemon_fixtures.h"
#include "test_filesystem.h"

namespace fs = test_fs;

static const char kTunnelCloseReasonOperatorClose[] = "operator_close";
static const unsigned long kTunnelFrameReadTimeoutMs = 30000UL;

fs::path make_test_root();

bool wait_until_true(const std::atomic<bool>& value, unsigned long timeout_ms);
void wait_past_resume_timeout(unsigned long resume_timeout_ms);

void initialize_state_with_port_forward_limits(
    TestDaemonState& state,
    const fs::path& root,
    const PortForwardLimitConfig& limits
);
void initialize_state_with_worker_limit(
    TestDaemonState& state,
    const fs::path& root,
    unsigned long max_workers
);
void initialize_state(TestDaemonState& state, const fs::path& root);
void enable_sandbox(TestDaemonState& state);

const char* tunnel_frame_type_name(PortTunnelFrameType type);
PortTunnelFrame read_tunnel_frame(SOCKET socket);
PortTunnelFrame read_tunnel_frame_for_phase(
    SOCKET socket,
    const char* phase,
    unsigned long timeout_ms
);
PortTunnelFrame expect_tunnel_frame(
    SOCKET socket,
    PortTunnelFrameType expected_type,
    const char* phase,
    unsigned long timeout_ms = kTunnelFrameReadTimeoutMs
);
void send_tunnel_frame(SOCKET socket, const PortTunnelFrame& frame);
bool try_read_tunnel_frame_with_timeout(
    SOCKET socket,
    unsigned long timeout_ms,
    PortTunnelFrame* frame
);
bool socket_readable_within(SOCKET socket, unsigned long timeout_ms);
void assert_socket_closed_within(SOCKET socket, unsigned long timeout_ms);
bool tcp_listener_has_pending_connection(SOCKET socket, unsigned long timeout_ms);

void assert_tunnel_error_code(const PortTunnelFrame& frame, const std::string& code);
void assert_forward_drop(
    const PortTunnelFrame& frame,
    const std::string& kind,
    const std::string& reason
);

PortTunnelFrame json_frame(PortTunnelFrameType type, uint32_t stream_id, const Json& meta);
PortTunnelFrame data_frame(
    PortTunnelFrameType type,
    uint32_t stream_id,
    const std::vector<unsigned char>& data
);
PortTunnelFrame empty_frame(PortTunnelFrameType type, uint32_t stream_id);
Json tunnel_open_meta(
    const std::string& role,
    const std::string& protocol,
    uint64_t generation,
    const std::string& resume_session_id = std::string()
);

void open_tunnel(TestDaemonState& state, UniqueSocket* client_socket, std::thread* server_thread);
PortTunnelFrame open_v4_tunnel(
    TestDaemonState& state,
    UniqueSocket* client_socket,
    std::thread* server_thread,
    const std::string& role,
    const std::string& protocol,
    uint64_t generation,
    const std::string& resume_session_id = std::string()
);
void close_tunnel(UniqueSocket* client_socket, std::thread* server_thread);
void wait_until_bindable(const std::string& endpoint);

void assert_http_streaming_routes(TestHttpConnectionHarness& harness, const fs::path& root);
void assert_first_request_callback_waits_for_request(TestHttpConnectionHarness& harness);
void assert_tunnel_rejects_invalid_requests(TestDaemonState& state);
void assert_tunnel_open_ready_and_limits(TestDaemonState& state);
void assert_tunnel_tcp_listener_and_connect_paths(TestDaemonState& state);
void assert_tunnel_udp_paths(TestDaemonState& state);
void assert_listen_session_rejects_second_retained_open(TestDaemonState& state);
void assert_tunnel_limit_and_pressure_paths(TestDaemonState& state);
void assert_tunnel_resume_and_expiry_paths(TestDaemonState& state);
void assert_detached_session_releases_active_tcp_accept_budget(const fs::path& root);
