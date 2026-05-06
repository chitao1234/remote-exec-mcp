#include <cassert>
#include <thread>

#include <netdb.h>
#include <sys/socket.h>

#include "platform.h"
#include "port_forward.h"
#include "port_forward_endpoint.h"

static SOCKET connect_socket_to(const std::string& endpoint) {
    const ParsedPortForwardEndpoint parsed = parse_port_forward_endpoint(endpoint);

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;

    addrinfo* result = NULL;
    assert(getaddrinfo(parsed.host.c_str(), parsed.port.c_str(), &hints, &result) == 0);

    SOCKET socket_value = INVALID_SOCKET;
    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        socket_value = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (socket_value == INVALID_SOCKET) {
            continue;
        }
        if (connect(socket_value, current->ai_addr, static_cast<int>(current->ai_addrlen)) == 0) {
            break;
        }
        close_socket(socket_value);
        socket_value = INVALID_SOCKET;
    }

    freeaddrinfo(result);
    assert(socket_value != INVALID_SOCKET);
    return socket_value;
}

int main() {
    NetworkSession network;
    PortForwardStore store;

    const Json listener = store.listen("127.0.0.1:0", "tcp", "", 0);
    const std::string bind_id = listener.at("bind_id").get<std::string>();

    bool closed_during_accept = false;
    std::thread accept_thread([&]() {
        try {
            (void)store.listen_accept(bind_id);
        } catch (const PortForwardError& ex) {
            closed_during_accept = ex.code() == "port_bind_closed";
        }
    });
    platform::sleep_ms(50UL);
    (void)store.listen_close(bind_id);
    accept_thread.join();
    assert(closed_during_accept);

    const Json read_listener = store.listen("127.0.0.1:0", "tcp", "", 0);
    const std::string read_bind_id = read_listener.at("bind_id").get<std::string>();
    UniqueSocket accepted_client(connect_socket_to(read_listener.at("endpoint").get<std::string>()));
    const Json accepted = store.listen_accept(read_bind_id);
    const std::string connection_id = accepted.at("connection_id").get<std::string>();

    bool closed_during_read = false;
    std::thread read_thread([&]() {
        try {
            (void)store.connection_read(connection_id);
        } catch (const PortForwardError& ex) {
            closed_during_read = ex.code() == "port_connection_closed";
        }
    });
    platform::sleep_ms(50UL);
    (void)store.connection_close(connection_id);
    read_thread.join();
    assert(closed_during_read);
}
