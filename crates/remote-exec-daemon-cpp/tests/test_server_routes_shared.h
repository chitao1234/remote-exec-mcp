#pragma once

#include <string>

#include "test_daemon_fixtures.h"
#include "test_filesystem.h"

test_fs::path make_server_routes_test_root(const std::string& directory_name);
DaemonConfig make_server_routes_test_config(const test_fs::path& root);
void run_platform_neutral_server_route_tests(TestRouteHarness& harness, const test_fs::path& root);
