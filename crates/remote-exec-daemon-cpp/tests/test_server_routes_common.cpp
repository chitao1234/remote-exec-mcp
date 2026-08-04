#include "test_server_routes_shared.h"

int main() {
    const test_fs::path root = make_daemon_test_root("remote-exec-cpp-server-routes-common-test");
    TestRouteHarness harness(root);

    run_platform_neutral_server_route_tests(harness, root);
    return 0;
}
