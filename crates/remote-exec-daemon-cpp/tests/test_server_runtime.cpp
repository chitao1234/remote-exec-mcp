#include "test_assert.h"
#include <cstdint>
#include <cstring>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <netinet/in.h>
#endif

#include "http/server_transport.h"
#include "platform/platform.h"
#include "runtime/server_runtime.h"
#include "test_daemon_fixtures.h"
#include "test_filesystem.h"
#include "test_socket_pair.h"
#include "tls/tls_connection_transport.h"

namespace fs = test_fs;

static const unsigned long TEST_TIMEOUT_MS = 1000UL;

static std::unique_ptr<ServerRuntime> start_runtime(
    const DaemonConfig& config,
    unsigned short* port
);

static DaemonConfig make_runtime_test_config(const fs::path& root) {
    DaemonConfig config = make_test_daemon_config(root);
    config.port_forward_limits.tunnel_io_timeout_ms = 100UL;
    return config;
}

static std::string read_http_head_from_socket(SOCKET socket) {
    set_socket_timeout_ms(socket, TEST_TIMEOUT_MS);
    std::string response;
    while (response.find("\r\n\r\n") == std::string::npos) {
        char ch = '\0';
        const int received = recv(socket, &ch, 1, 0);
        TEST_ASSERT(received > 0);
        response.push_back(ch);
    }
    set_socket_timeout_ms(socket, 0UL);
    return response;
}

static bool wait_for_active_connections(
    ConnectionManager& manager,
    unsigned long expected,
    unsigned long timeout_ms
) {
    const std::uint64_t deadline = platform::monotonic_ms() + timeout_ms;
    while (platform::monotonic_ms() < deadline) {
        if (manager.active_count() == expected) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return manager.active_count() == expected;
}

static void request_shutdown_and_join_quickly(ServerRuntime& runtime) {
    const std::uint64_t started_at_ms = platform::monotonic_ms();
    runtime.request_shutdown();
    runtime.join();
    TEST_ASSERT(platform::monotonic_ms() - started_at_ms < TEST_TIMEOUT_MS);
}

static SOCKET connect_client(unsigned short port) {
    std::ostringstream service;
    service << port;

    SocketAddressQuery query;
    query.family = AF_INET;
    query.socktype = SOCK_STREAM;
    query.protocol = IPPROTO_TCP;

    std::vector<SocketAddress> addresses;
    std::string resolve_error;
    TEST_ASSERT(resolve_socket_addresses(
        "127.0.0.1",
        service.str().c_str(),
        query,
        &addresses,
        &resolve_error
    ));

    SOCKET client = INVALID_SOCKET;
    for (std::size_t i = 0; i < addresses.size(); ++i) {
        const SocketAddress& current = addresses[i];
        client = socket(current.family, current.socktype, current.protocol);
        if (client == INVALID_SOCKET) {
            continue;
        }
        if (connect_socket(client, current.sockaddr_ptr(), current.address_len) == 0) {
            break;
        }
        close_socket(client);
        client = INVALID_SOCKET;
    }

    TEST_ASSERT(client != INVALID_SOCKET);
    return client;
}

static void assert_health_request(ServerRuntime& runtime, unsigned short port) {
    UniqueSocket client(connect_client(port));
    TEST_ASSERT(wait_for_active_connections(runtime.connection_manager(), 1UL, TEST_TIMEOUT_MS));

    send_all(
        client.get(),
        "POST /v1/health HTTP/1.1\r\n"
        "Connection: close\r\n"
        "Content-Length: 0\r\n"
        "\r\n"
    );

    const std::string response = read_all_from_socket(client.get());
    TEST_ASSERT(response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(response.find("\"status\":\"ok\"") != std::string::npos);
    TEST_ASSERT(wait_for_active_connections(runtime.connection_manager(), 0UL, TEST_TIMEOUT_MS));
}

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
static std::string read_all_from_transport(ConnectionTransport& client) {
    std::string output;
    char buffer[4096];
    for (;;) {
        TEST_ASSERT(client.wait_readable(TEST_TIMEOUT_MS) > 0);
        const int received = client.read(buffer, sizeof(buffer));
        if (received <= 0) {
            break;
        }
        output.append(buffer, static_cast<std::size_t>(received));
    }
    return output;
}

static void assert_tls_health_request(const DaemonConfig& base_config) {
    DaemonConfig config = base_config;
    config.transport = Transport::Tls;
    config.tls_cert_pem = "tests/fixtures/tls/server.pem";
    config.tls_key_pem = "tests/fixtures/tls/server.key";
    config.tls_ca_pem = "tests/fixtures/tls/ca.pem";
    config.tls_pinned_client_cert_pem = "tests/fixtures/tls/client.pem";

    unsigned short port = 0;
    std::unique_ptr<ServerRuntime> runtime = start_runtime(config, &port);
    UniqueSocket socket(connect_client(port));
    DaemonConfig client_config;
    client_config.reverse_tls_cert_pem = "tests/fixtures/tls/client.pem";
    client_config.reverse_tls_key_pem = "tests/fixtures/tls/client.key";
    client_config.reverse_tls_ca_pem = "tests/fixtures/tls/ca.pem";
    const std::shared_ptr<TlsContext> context = make_tls_client_context(client_config);
    std::shared_ptr<ConnectionTransport> client = make_tls_client_connection_transport(
        std::move(socket),
        context,
        "localhost",
        TEST_TIMEOUT_MS
    );
    TEST_ASSERT(wait_for_active_connections(runtime->connection_manager(), 1UL, TEST_TIMEOUT_MS));

    send_all(
        *client,
        "POST /v1/health HTTP/1.1\r\n"
        "Connection: close\r\n"
        "Content-Length: 0\r\n"
        "\r\n"
    );
    const std::string response = read_all_from_transport(*client);
    TEST_ASSERT(response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(response.find("\"status\":\"ok\"") != std::string::npos);
    TEST_ASSERT(wait_for_active_connections(runtime->connection_manager(), 0UL, TEST_TIMEOUT_MS));
    runtime->request_shutdown();
    runtime->join();
}
#endif

static std::unique_ptr<ServerRuntime> start_runtime(
    const DaemonConfig& config,
    unsigned short* port
) {
    std::unique_ptr<ServerRuntime> runtime(new ServerRuntime(config));
    runtime->start_accept_loop();
    *port = runtime->bound_port();
    TEST_ASSERT(*port != 0);
    return runtime;
}

static void assert_runtime_shutdown_with_idle_connection(const DaemonConfig& config) {
    unsigned short port = 0;
    std::unique_ptr<ServerRuntime> runtime = start_runtime(config, &port);
    UniqueSocket client(connect_client(port));
    TEST_ASSERT(wait_for_active_connections(runtime->connection_manager(), 1UL, TEST_TIMEOUT_MS));

    request_shutdown_and_join_quickly(*runtime);
    TEST_ASSERT(runtime->connection_manager().active_count() == 0UL);
}

static void assert_runtime_shutdown_with_blocked_request_body(const DaemonConfig& config) {
    unsigned short port = 0;
    std::unique_ptr<ServerRuntime> runtime = start_runtime(config, &port);
    UniqueSocket client(connect_client(port));
    TEST_ASSERT(wait_for_active_connections(runtime->connection_manager(), 1UL, TEST_TIMEOUT_MS));

    send_all(
        client.get(),
        "POST /v1/health HTTP/1.1\r\n"
        "Content-Length: 100\r\n"
        "\r\n"
        "partial-body"
    );

    request_shutdown_and_join_quickly(*runtime);
    TEST_ASSERT(runtime->connection_manager().active_count() == 0UL);
}

static void assert_runtime_shutdown_with_upgraded_tunnel_connection(const DaemonConfig& config) {
    unsigned short port = 0;
    std::unique_ptr<ServerRuntime> runtime = start_runtime(config, &port);
    UniqueSocket client(connect_client(port));
    TEST_ASSERT(wait_for_active_connections(runtime->connection_manager(), 1UL, TEST_TIMEOUT_MS));

    send_all(
        client.get(),
        "POST /v1/port/tunnel HTTP/1.1\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: remote-exec-port-tunnel\r\n"
        "X-Remote-Exec-Port-Tunnel-Version: 4\r\n"
        "Content-Length: 0\r\n"
        "\r\n"
    );
    const std::string response = read_http_head_from_socket(client.get());
    TEST_ASSERT(response.find("HTTP/1.1 101 Switching Protocols\r\n") == 0);

    request_shutdown_and_join_quickly(*runtime);
    TEST_ASSERT(runtime->connection_manager().active_count() == 0UL);
}

int main() {
    NetworkSession network;
    const fs::path root = fs::unique_test_root("remote-exec-cpp-server-runtime-test");
    fs::remove_all(root);
    fs::create_directories(root);

    const DaemonConfig config = make_runtime_test_config(root);

    {
        unsigned short port = 0;
        std::unique_ptr<ServerRuntime> runtime = start_runtime(config, &port);
        assert_health_request(*runtime, port);

        runtime->request_shutdown();
        runtime->maintenance_once();
        TEST_ASSERT(runtime->connection_manager().active_count() == 0UL);
        runtime->join();
        TEST_ASSERT(runtime->connection_manager().active_count() == 0UL);
    }

    assert_runtime_shutdown_with_idle_connection(config);
    assert_runtime_shutdown_with_blocked_request_body(config);
    assert_runtime_shutdown_with_upgraded_tunnel_connection(config);
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
    assert_tls_health_request(config);
#endif
}
