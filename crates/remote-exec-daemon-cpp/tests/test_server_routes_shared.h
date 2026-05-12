#pragma once

#include <string>

#include "server.h"
#include "test_filesystem.h"

test_fs::path make_server_routes_test_root(const std::string& directory_name);
void initialize_server_routes_state(AppState& state, const test_fs::path& root);
void run_platform_neutral_server_route_tests(AppState& state, const test_fs::path& root);
