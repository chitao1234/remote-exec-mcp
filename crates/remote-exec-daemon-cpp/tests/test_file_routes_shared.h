#pragma once

#include "test_daemon_fixtures.h"
#include "test_filesystem.h"

// Shared file tool route coverage. Kept platform-neutral so the POSIX and
// Windows-capable server route test binaries both exercise it.
void run_file_tool_route_tests(TestRouteHarness& harness, const test_fs::path& root);
