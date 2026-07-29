#include "test_assert.h"

#include <exception>
#include <memory>
#include <stdexcept>
#include <string>
#include <thread>

#include "core/config.h"
#include "http/connection_transport.h"
#include "test_socket_pair.h"
#include "tls/tls_connection_transport.h"

namespace {

const unsigned long TLS_TEST_TIMEOUT_MS = 5000UL;
const char TLS_FIXTURE_DIR[] = "tests/fixtures/tls/";

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL

DaemonConfig make_server_config(const std::string& pinned_client) {
    DaemonConfig config;
    config.transport = Transport::Tls;
    config.tls_cert_pem = std::string(TLS_FIXTURE_DIR) + "server.pem";
    config.tls_key_pem = std::string(TLS_FIXTURE_DIR) + "server.key";
    config.tls_ca_pem = std::string(TLS_FIXTURE_DIR) + "ca.pem";
    config.tls_pinned_client_cert_pem = pinned_client;
    return config;
}

DaemonConfig make_client_config() {
    DaemonConfig config;
    config.reverse_transport = Transport::Tls;
    config.reverse_tls_cert_pem = std::string(TLS_FIXTURE_DIR) + "client.pem";
    config.reverse_tls_key_pem = std::string(TLS_FIXTURE_DIR) + "client.key";
    config.reverse_tls_ca_pem = std::string(TLS_FIXTURE_DIR) + "ca.pem";
    config.reverse_tls_server_name = "localhost";
    return config;
}

std::string read_exact(ConnectionTransport& transport, std::size_t size) {
    std::string result(size, '\0');
    std::size_t offset = 0U;
    while (offset < size) {
        TEST_ASSERT(transport.wait_readable(TLS_TEST_TIMEOUT_MS) > 0);
        const int received = transport.read(&result[offset], size - offset);
        TEST_ASSERT(received > 0);
        offset += static_cast<std::size_t>(received);
    }
    return result;
}

struct TransportPair {
    std::shared_ptr<ConnectionTransport> server;
    std::shared_ptr<ConnectionTransport> client;
};

TransportPair connect_tls_pair(const DaemonConfig& server_config) {
    ConnectedSocketPair sockets = make_connected_socket_pair();
    const std::shared_ptr<TlsContext> server_context = make_tls_server_context(server_config);
    const DaemonConfig client_config = make_client_config();
    const std::shared_ptr<TlsContext> client_context = make_tls_client_context(client_config);

    std::shared_ptr<ConnectionTransport> server;
    std::exception_ptr server_error;
    std::thread server_thread([&]() {
        try {
            server = make_tls_server_connection_transport(
                std::move(sockets.first),
                server_context,
                TLS_TEST_TIMEOUT_MS
            );
        } catch (...) {
            server_error = std::current_exception();
        }
    });
    std::shared_ptr<ConnectionTransport> client = make_tls_client_connection_transport(
        std::move(sockets.second),
        client_context,
        client_config.reverse_tls_server_name,
        TLS_TEST_TIMEOUT_MS
    );
    server_thread.join();
    if (server_error) {
        std::rethrow_exception(server_error);
    }

    TransportPair pair;
    pair.server = server;
    pair.client = client;
    return pair;
}

void test_mutual_tls_full_duplex() {
    const std::string pinned_client = std::string(TLS_FIXTURE_DIR) + "client.pem";
    TransportPair pair = connect_tls_pair(make_server_config(pinned_client));

    const std::string server_payload(128U * 1024U, 's');
    const std::string client_payload(128U * 1024U, 'c');
    int server_written = 0;
    int client_written = 0;
    std::thread server_writer([&]() {
        server_written = pair.server->write(server_payload.data(), server_payload.size());
    });
    std::thread client_writer([&]() {
        client_written = pair.client->write(client_payload.data(), client_payload.size());
    });

    TEST_ASSERT(read_exact(*pair.server, client_payload.size()) == client_payload);
    TEST_ASSERT(read_exact(*pair.client, server_payload.size()) == server_payload);
    server_writer.join();
    client_writer.join();
    TEST_ASSERT(server_written == static_cast<int>(server_payload.size()));
    TEST_ASSERT(client_written == static_cast<int>(client_payload.size()));

    pair.server->shutdown();
    pair.client->shutdown();
}

void test_pinned_client_mismatch() {
    const std::string other_client = std::string(TLS_FIXTURE_DIR) + "other-client.pem";
    bool rejected = false;
    try {
        TransportPair pair = connect_tls_pair(make_server_config(other_client));
        pair.server->shutdown();
        pair.client->shutdown();
    } catch (const std::exception& ex) {
        rejected = std::string(ex.what()).find("pinned TLS peer certificate mismatch")
                   != std::string::npos;
    }
    TEST_ASSERT(rejected);
}

#else

void test_tls_disabled_error() {
    bool rejected = false;
    try {
        (void)make_tls_server_context(DaemonConfig());
    } catch (const std::exception& ex) {
        rejected = std::string(ex.what()).find("TLS=openssl") != std::string::npos;
    }
    TEST_ASSERT(rejected);
}

#endif

} // namespace

int main() {
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
    test_mutual_tls_full_duplex();
    test_pinned_client_mismatch();
#else
    test_tls_disabled_error();
#endif
    return 0;
}
