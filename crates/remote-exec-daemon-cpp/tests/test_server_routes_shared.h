#pragma once

#include <string>

#include "server.h"
#include "test_filesystem.h"

test_fs::path make_server_routes_test_root(const std::string& directory_name);
inline DaemonConfig make_server_routes_test_config(const test_fs::path& root) {
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
void initialize_server_routes_state(AppState& state, const test_fs::path& root);
void run_platform_neutral_server_route_tests(AppState& state, const test_fs::path& root);
