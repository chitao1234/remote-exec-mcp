#include "test_server_routes_shared.h"

int main() {
    const test_fs::path root =
        make_server_routes_test_root("remote-exec-cpp-server-routes-common-test");
    AppState state;
    initialize_server_routes_state(state, root);

    run_platform_neutral_server_route_tests(state, root);
    return 0;
}
