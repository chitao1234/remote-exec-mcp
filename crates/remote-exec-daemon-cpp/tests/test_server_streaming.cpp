#include "test_server_streaming_shared.h"

int main() {
    NetworkSession network;
    const fs::path root = make_test_root();
    AppState state;
    initialize_state(state, root);

    assert_http_streaming_routes(state, root);
    assert_tunnel_rejects_invalid_requests(state);
    assert_tunnel_open_ready_and_limits(state);
    assert_tunnel_tcp_listener_and_connect_paths(state);
    assert_tunnel_udp_paths(state);
    assert_listen_session_rejects_second_retained_open(state);
    assert_tunnel_limit_and_pressure_paths(state);
    assert_tunnel_resume_and_expiry_paths(state);
    assert_detached_session_releases_active_tcp_accept_budget(root);

    return 0;
}
