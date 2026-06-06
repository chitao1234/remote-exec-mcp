#include "test_daemon_fixtures.h"

#include "capabilities/daemon_capabilities.h"
#include "platform/platform.h"
#include "port_forward/port_tunnel.h"

namespace {

std::string stable_test_shell() {
#ifdef _WIN32
    return platform::resolve_default_shell("");
#else
    return platform::resolve_default_shell("/bin/sh");
#endif
}

} // namespace

ServerRouteContext make_test_server_route_context(TestDaemonState& state) {
    return make_server_route_context(state.config, state.metadata, state.sandbox, state.services);
}

HttpConnectionContext make_test_http_connection_context(TestDaemonState& state) {
    return make_http_connection_context(state.config, state.metadata, state.sandbox, state.services, state.shutdown);
}

TestRouteHarness::TestRouteHarness(const test_fs::path& root) : state(), routes() {
    initialize_test_daemon_state(state, root);
    refresh_context();
}

void TestRouteHarness::refresh_context() {
    routes = make_test_server_route_context(state);
}

TestHttpConnectionHarness::TestHttpConnectionHarness(const test_fs::path& root) : state(), connection() {
    initialize_test_daemon_state(state, root);
    refresh_context();
}

void TestHttpConnectionHarness::refresh_context() {
    connection = make_test_http_connection_context(state);
}

test_fs::path make_daemon_test_root(const std::string& directory_name) {
    const test_fs::path root = test_fs::unique_test_root(directory_name);
    test_fs::remove_all(root);
    test_fs::create_directories(root);
    return root;
}

DaemonConfig make_test_daemon_config(const test_fs::path& root) {
    DaemonConfig config;
    config.target = "cpp-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = root.string();
    config.default_shell.clear();
    config.allow_login_shell = true;
    config.http_auth_bearer_token.clear();
    config.max_request_header_bytes = 65536;
    config.max_request_body_bytes = 536870912;
    config.transfer_limits = default_transfer_limit_config();
    config.max_open_sessions = 64;
    return config;
}

void initialize_test_daemon_state_with_port_forward_limits(TestDaemonState& state,
                                                           const test_fs::path& root,
                                                           const PortForwardLimitConfig& limits) {
    state.config = make_test_daemon_config(root);
    state.config.port_forward_limits = limits;
    state.metadata.daemon_instance_id = "test-instance";
    state.metadata.hostname = "test-host";
    state.metadata.default_shell = stable_test_shell();
    state.metadata.capabilities = detect_daemon_capabilities();
    state.services.port_tunnel = create_port_tunnel_service(limits);
}

void initialize_test_daemon_state_with_worker_limit(TestDaemonState& state,
                                                    const test_fs::path& root,
                                                    unsigned long max_workers) {
    PortForwardLimitConfig limits;
    limits.max_worker_threads = max_workers;
    initialize_test_daemon_state_with_port_forward_limits(state, root, limits);
}

void initialize_test_daemon_state(TestDaemonState& state, const test_fs::path& root) {
    initialize_test_daemon_state_with_worker_limit(state, root, DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
}

void enable_test_daemon_sandbox(TestDaemonState& state) {
    state.sandbox.enabled = state.config.sandbox_configured;
    if (state.sandbox.enabled) {
        state.sandbox.compiled = compile_filesystem_sandbox(state.config.sandbox);
    }
}

HttpRequest make_json_http_request(const std::string& path, const Json& body) {
    HttpRequest request;
    request.method = "POST";
    request.path = path;
    request.headers["content-type"] = "application/json";
    request.body = body.dump();
    return request;
}
