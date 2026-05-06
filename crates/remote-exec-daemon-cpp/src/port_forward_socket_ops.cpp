#include "port_forward_socket_ops.h"

#include <cstring>
#include <sstream>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#else
#include <arpa/inet.h>
#include <netdb.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "port_forward_endpoint.h"
#include "port_forward_error.h"

namespace {

int protocol_to_socktype(const std::string& protocol) {
    if (protocol == "tcp") {
        return SOCK_STREAM;
    }
    if (protocol == "udp") {
        return SOCK_DGRAM;
    }
    throw PortForwardError(
        400,
        "bad_request",
        "unsupported port forward protocol `" + protocol + "`"
    );
}

int protocol_to_ipproto(const std::string& protocol) {
    if (protocol == "tcp") {
        return IPPROTO_TCP;
    }
    if (protocol == "udp") {
        return IPPROTO_UDP;
    }
    throw PortForwardError(
        400,
        "bad_request",
        "unsupported port forward protocol `" + protocol + "`"
    );
}

ParsedPortForwardEndpoint endpoint_to_host_port(const std::string& endpoint) {
    const ParsedPortForwardEndpoint parsed = parse_port_forward_endpoint(endpoint);
    if (parsed.host.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint host must not be empty");
    }
    parse_port_number(parsed.port);
    return parsed;
}

addrinfo* resolve_endpoint(
    const std::string& endpoint,
    const std::string& protocol,
    int flags,
    const char* error_code
) {
    const ParsedPortForwardEndpoint parsed = endpoint_to_host_port(endpoint);

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = protocol_to_socktype(protocol);
    hints.ai_protocol = protocol_to_ipproto(protocol);
    hints.ai_flags = flags;

    addrinfo* result = NULL;
    const int status = getaddrinfo(parsed.host.c_str(), parsed.port.c_str(), &hints, &result);
    if (status != 0 || result == NULL) {
        std::ostringstream message;
        message << "resolving endpoint `" << endpoint << "` failed";
#ifdef _WIN32
        message << ": " << status;
#else
        message << ": " << gai_strerror(status);
#endif
        throw PortForwardError(400, error_code, message.str());
    }
    return result;
}

}  // namespace

std::string printable_port_forward_endpoint(
    const sockaddr* address,
    socklen_t address_len
) {
    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    const int result = getnameinfo(
        address,
        address_len,
        host,
        sizeof(host),
        service,
        sizeof(service),
        NI_NUMERICHOST | NI_NUMERICSERV
    );
    if (result != 0) {
        return "unknown:0";
    }

    if (address->sa_family == AF_INET6) {
        return "[" + std::string(host) + "]:" + std::string(service);
    }
    return std::string(host) + ":" + std::string(service);
}

std::string socket_local_endpoint(SOCKET socket) {
    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    socklen_t address_len = sizeof(address);
    if (getsockname(socket, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        throw PortForwardError(400, "port_bind_failed", socket_error_message("getsockname"));
    }
    return printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&address), address_len);
}

SOCKET bind_port_forward_socket(const std::string& endpoint, const std::string& protocol) {
    addrinfo* result = resolve_endpoint(endpoint, protocol, AI_PASSIVE, "invalid_endpoint");
    SOCKET bound_socket = INVALID_SOCKET;

    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        bound_socket = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (bound_socket == INVALID_SOCKET) {
            continue;
        }

        int yes = 1;
        setsockopt(
            bound_socket,
            SOL_SOCKET,
            SO_REUSEADDR,
            reinterpret_cast<const char*>(&yes),
            sizeof(yes)
        );

        if (bind(bound_socket, current->ai_addr, static_cast<int>(current->ai_addrlen)) == 0) {
            break;
        }

        close_socket(bound_socket);
        bound_socket = INVALID_SOCKET;
    }

    freeaddrinfo(result);

    if (bound_socket == INVALID_SOCKET) {
        throw PortForwardError(400, "port_bind_failed", socket_error_message("bind"));
    }

    if (protocol == "tcp" && listen(bound_socket, SOMAXCONN) != 0) {
        const std::string message = socket_error_message("listen");
        close_socket(bound_socket);
        throw PortForwardError(400, "port_bind_failed", message);
    }

    return bound_socket;
}

SOCKET connect_port_forward_socket(const std::string& endpoint, const std::string& protocol) {
    addrinfo* result = resolve_endpoint(endpoint, protocol, 0, "invalid_endpoint");
    SOCKET connected_socket = INVALID_SOCKET;

    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        connected_socket = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (connected_socket == INVALID_SOCKET) {
            continue;
        }

        if (connect(
                connected_socket,
                current->ai_addr,
                static_cast<int>(current->ai_addrlen)
            ) == 0) {
            break;
        }

        close_socket(connected_socket);
        connected_socket = INVALID_SOCKET;
    }

    freeaddrinfo(result);

    if (connected_socket == INVALID_SOCKET) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("connect"));
    }

    return connected_socket;
}

void send_all_socket(SOCKET socket, const std::string& data) {
    std::size_t offset = 0;
    while (offset < data.size()) {
        const int sent = send(
            socket,
            data.data() + offset,
            static_cast<int>(data.size() - offset),
            0
        );
        if (sent <= 0) {
            throw PortForwardError(400, "port_write_failed", socket_error_message("send"));
        }
        offset += static_cast<std::size_t>(sent);
    }
}

sockaddr_storage parse_port_forward_peer(const std::string& peer, socklen_t* peer_len) {
    addrinfo* result = resolve_endpoint(peer, "udp", 0, "invalid_endpoint");
    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    *peer_len = 0;
    if (result != NULL) {
        std::memcpy(&address, result->ai_addr, result->ai_addrlen);
        *peer_len = static_cast<socklen_t>(result->ai_addrlen);
    }
    freeaddrinfo(result);
    return address;
}
