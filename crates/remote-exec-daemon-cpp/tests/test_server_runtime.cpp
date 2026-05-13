#include <cassert>
#include <cstdint>
#include <cstring>
#include <sstream>
#include <string>

#ifdef _WIN32
#include <ws2tcpip.h>
#else
#include <netdb.h>
#include <sys/socket.h>
#endif

#include "platform.h"
#include "server_runtime.h"
#include "test_filesystem.h"

namespace fs = test_fs;

static const unsigned long TEST_TIMEOUT_MS = 1000UL;

static std::string read_all_from_socket(SOCKET socket) {
    std::string output;
    char buffer[4096];
    for (;;) {
        const int received = recv(socket, buffer, sizeof(buffer), 0);
        if (received <= 0) {
            break;
        }
        output.append(buffer, static_cast<std::size_t>(received));
    }
    return output;
}

static bool wait_for_active_connections(ConnectionManager& manager, unsigned long expected, unsigned long timeout_ms) {
    const std::uint64_t deadline = platform::monotonic_ms() + timeout_ms;
    while (platform::monotonic_ms() < deadline) {
        if (manager.active_count() == expected) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return manager.active_count() == expected;
}

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

static void assert_health_request(ServerRuntime& runtime, unsigned short port) {
    UniqueSocket client(connect_client(port));
    assert(wait_for_active_connections(runtime.connection_manager(), 1UL, TEST_TIMEOUT_MS));

    send_all(client.get(),
             "POST /v1/health HTTP/1.1\r\n"
             "Connection: close\r\n"
             "Content-Length: 0\r\n"
             "\r\n");

    const std::string response = read_all_from_socket(client.get());
    assert(response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(response.find("\"status\":\"ok\"") != std::string::npos);
    assert(wait_for_active_connections(runtime.connection_manager(), 0UL, TEST_TIMEOUT_MS));
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
    config.max_request_header_bytes = DEFAULT_MAX_REQUEST_HEADER_BYTES;
    config.max_request_body_bytes = DEFAULT_MAX_REQUEST_BODY_BYTES;
    config.max_open_sessions = DEFAULT_MAX_OPEN_SESSIONS;

    ServerRuntime runtime(config);
    runtime.start_accept_loop();
    const unsigned short port = runtime.bound_port();
    assert(port != 0);
    assert_health_request(runtime, port);

    runtime.request_shutdown();
    runtime.maintenance_once();
    assert(runtime.connection_manager().active_count() == 0UL);
    runtime.join();
    assert(runtime.connection_manager().active_count() == 0UL);
}
