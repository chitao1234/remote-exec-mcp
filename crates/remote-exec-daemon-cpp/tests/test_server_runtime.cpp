#include <cassert>
#include <filesystem>
#include <sstream>

#include <netdb.h>
#include <sys/socket.h>

#include "platform.h"
#include "server_runtime.h"

namespace fs = std::filesystem;

static SOCKET connect_client(unsigned short port) {
    std::ostringstream service;
    service << port;

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;

    addrinfo* result = NULL;
    assert(getaddrinfo("127.0.0.1", service.str().c_str(), &hints, &result) == 0);

    SOCKET client = INVALID_SOCKET;
    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        client = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (client == INVALID_SOCKET) {
            continue;
        }
        if (connect(client, current->ai_addr, static_cast<int>(current->ai_addrlen)) == 0) {
            break;
        }
        close_socket(client);
        client = INVALID_SOCKET;
    }

    freeaddrinfo(result);
    assert(client != INVALID_SOCKET);
    return client;
}

int main() {
    NetworkSession network;
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-server-runtime-test";
    fs::remove_all(root);
    fs::create_directories(root);

    DaemonConfig config;
    config.target = "cpp-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = root.string();
    config.default_shell.clear();
    config.allow_login_shell = true;
    config.max_request_header_bytes = 65536;
    config.max_request_body_bytes = 65536;
    config.max_open_sessions = 64;
    config.port_forward_limits = default_port_forward_limit_config();
    config.yield_time = default_yield_time_config();

    ServerRuntime runtime(config);
    runtime.start_accept_loop();
    const unsigned short port = runtime.bound_port();
    assert(port != 0);

    UniqueSocket client(connect_client(port));
    const std::uint64_t deadline = platform::monotonic_ms() + 1000UL;
    while (runtime.connection_manager().active_count() == 0UL &&
           platform::monotonic_ms() < deadline) {
        platform::sleep_ms(10UL);
    }
    assert(runtime.connection_manager().active_count() == 1UL);

    runtime.request_shutdown();
    runtime.maintenance_once();
    assert(runtime.connection_manager().active_count() == 0UL);
    runtime.join();
    assert(runtime.connection_manager().active_count() == 0UL);
}
