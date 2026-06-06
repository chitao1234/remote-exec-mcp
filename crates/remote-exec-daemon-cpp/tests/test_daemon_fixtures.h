#pragma once

#include <string>
#include <utility>

#include "http/http_connection.h"
#include "http/http_helpers.h"
#include "rpc/server_routes.h"
#include "runtime/server.h"
#include "test_filesystem.h"

test_fs::path make_daemon_test_root(const std::string& directory_name);
DaemonConfig make_test_daemon_config(const test_fs::path& root);
void initialize_test_daemon_state(AppState& state, const test_fs::path& root);
void initialize_test_daemon_state_with_port_forward_limits(AppState& state,
                                                           const test_fs::path& root,
                                                           const PortForwardLimitConfig& limits);
void initialize_test_daemon_state_with_worker_limit(AppState& state,
                                                    const test_fs::path& root,
                                                    unsigned long max_workers);
void enable_test_daemon_sandbox(AppState& state);
HttpRequest make_json_http_request(const std::string& path, const Json& body);

inline HttpResponse route_request(AppState& state, const HttpRequest& request) {
    ServerRouteContext context = make_server_route_context(state);
    return route_request(context, request);
}

inline void handle_client(AppState& state, UniqueSocket client) {
    handle_client(make_http_connection_context(state), std::move(client));
}
