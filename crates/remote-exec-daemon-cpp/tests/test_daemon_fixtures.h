#pragma once

#include <memory>
#include <string>
#include <utility>

#include "http/http_connection.h"
#include "http/http_helpers.h"
#include "rpc/server_routes.h"
#include "runtime/app_context.h"
#include "test_filesystem.h"

struct TestDaemonState {
    DaemonConfig config;
    AppMetadata metadata;
    AppSandboxState sandbox;
    AppServices services;
    AppShutdownState shutdown;
};

ServerRouteContext make_test_server_route_context(TestDaemonState& state);
HttpConnectionContext make_test_http_connection_context(TestDaemonState& state);

struct TestRouteHarness {
    TestDaemonState state;
    std::unique_ptr<ServerRouteContext> routes;

    explicit TestRouteHarness(const test_fs::path& root);
    void refresh_context();
};

struct TestHttpConnectionHarness {
    TestDaemonState state;
    std::unique_ptr<HttpConnectionContext> connection;

    explicit TestHttpConnectionHarness(const test_fs::path& root);
    void refresh_context();
};

test_fs::path make_daemon_test_root(const std::string& directory_name);
DaemonConfig make_test_daemon_config(const test_fs::path& root);
void initialize_test_daemon_state(TestDaemonState& state, const test_fs::path& root);
void initialize_test_daemon_state_with_port_forward_limits(
    TestDaemonState& state,
    const test_fs::path& root,
    const PortForwardLimitConfig& limits
);
void initialize_test_daemon_state_with_worker_limit(
    TestDaemonState& state,
    const test_fs::path& root,
    unsigned long max_workers
);
void enable_test_daemon_sandbox(TestDaemonState& state);
HttpRequest make_json_http_request(const std::string& path, const Json& body);

inline HttpResponse route_request(TestRouteHarness& harness, const HttpRequest& request) {
    return route_request(*harness.routes, request);
}

inline void handle_client(TestHttpConnectionHarness& harness, UniqueSocket client) {
    handle_client(*harness.connection, std::move(client));
}
