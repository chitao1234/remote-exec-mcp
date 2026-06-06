#pragma once

#include <string>

#include "http/http_helpers.h"
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
