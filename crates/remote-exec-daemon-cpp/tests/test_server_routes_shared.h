#pragma once

#include <string>

#include "test_daemon_fixtures.h"
#include "test_filesystem.h"

void run_platform_neutral_server_route_tests(TestRouteHarness& harness, const test_fs::path& root);
