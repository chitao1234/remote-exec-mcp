#include "port_forward/port_forward_socket_ops.h"

#include <cstring>
#include <sstream>
#include <vector>

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <arpa/inet.h>
#include <cerrno>
#include <netdb.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "port_forward/port_forward_endpoint.h"
#include "port_forward/port_forward_error.h"
#ifndef _WIN32
#include "platform/posix_fd.h"
#endif

namespace {

int protocol_to_socktype(const std::string& protocol) {
    if (protocol == "tcp") {
        return SOCK_STREAM;
    }
    if (protocol == "udp") {
        return SOCK_DGRAM;
    }
    throw PortForwardError(400, "bad_request", "unsupported port forward protocol `" + protocol + "`");
}

int protocol_to_ipproto(const std::string& protocol) {
    if (protocol == "tcp") {
        return IPPROTO_TCP;
    }
    if (protocol == "udp") {
        return IPPROTO_UDP;
    }
    throw PortForwardError(400, "bad_request", "unsupported port forward protocol `" + protocol + "`");
}

ParsedPortForwardEndpoint endpoint_to_host_port(const std::string& endpoint) {
    const ParsedPortForwardEndpoint parsed = parse_port_forward_endpoint(endpoint);
    if (parsed.host.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint host must not be empty");
    }
    parse_port_number(parsed.port);
    return parsed;
}

std::vector<SocketAddress>
resolve_endpoint(const std::string& endpoint, const std::string& protocol, bool passive, const char* error_code) {
    const ParsedPortForwardEndpoint parsed = endpoint_to_host_port(endpoint);
#ifdef REMOTE_EXEC_CPP_WINSOCK1
    if (parsed.host.find(':') != std::string::npos) {
        throw PortForwardError(400,
                               error_code,
                               "IPv6 endpoint `" + endpoint +
                                   "` is not supported by the Winsock 1 Windows build; use an IPv4 endpoint");
    }
#endif

    SocketAddressQuery query;
    query.family = AF_UNSPEC;
    query.socktype = protocol_to_socktype(protocol);
    query.protocol = protocol_to_ipproto(protocol);
    query.passive = passive;

    std::vector<SocketAddress> addresses;
    std::string resolve_error;
    if (!resolve_socket_addresses(parsed.host.c_str(), parsed.port.c_str(), query, &addresses, &resolve_error)) {
        const std::string operation = "resolving endpoint `" + endpoint + "`";
        std::ostringstream message;
        message << operation << " failed";
        if (!resolve_error.empty()) {
            message << ": " << resolve_error;
        }
        throw PortForwardError(400, error_code, message.str());
    }
    return addresses;
}

} // namespace

void set_socket_nonblocking(SOCKET socket, bool enabled) {
#ifdef _WIN32
    u_long mode = enabled ? 1UL : 0UL;
    if (ioctlsocket(socket, FIONBIO, &mode) != 0) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("ioctlsocket"));
    }
#else
    if (!posix_fd::set_nonblocking(socket, enabled)) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("fcntl"));
    }
#endif
}

namespace {
bool wait_for_connect(SOCKET socket, unsigned long timeout_ms) {
    const int selected = wait_socket_writable(socket, timeout_ms);
    if (selected < 0) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("poll"));
    }
    return selected > 0;
}

bool tcp_connect_with_timeout(SOCKET socket, const sockaddr* address, socklen_t address_len, unsigned long timeout_ms) {
    set_socket_nonblocking(socket, true);
    if (connect_socket(socket, address, address_len) == 0) {
        set_socket_nonblocking(socket, false);
        return true;
    }

    const int connect_error = last_socket_error();
    if (!connect_in_progress_socket_error(connect_error)) {
        set_socket_nonblocking(socket, false);
        return false;
    }

    if (!wait_for_connect(socket, timeout_ms)) {
        set_socket_nonblocking(socket, false);
        throw PortForwardError(400, "port_connect_failed", "tcp connect timed out");
    }

    int socket_error = 0;
    if (socket_error_option(socket, &socket_error) != 0) {
        set_socket_nonblocking(socket, false);
        throw PortForwardError(400, "port_connect_failed", socket_error_message("getsockopt"));
    }
    set_socket_nonblocking(socket, false);
    if (socket_error != 0) {
#ifdef _WIN32
        WSASetLastError(socket_error);
#else
        errno = socket_error;
#endif
        return false;
    }
    return true;
}

} // namespace

SOCKET accept_port_forward_peer(SOCKET listener, sockaddr* peer_address, socklen_t* peer_len) {
    return accept_socket_cloexec(listener, peer_address, peer_len);
}

int recv_port_forward_datagram(SOCKET socket,
                               char* data,
                               std::size_t size,
                               sockaddr* peer_address,
                               socklen_t* peer_len) {
    return recvfrom_bounded(socket, data, size, peer_address, peer_len);
}

int send_port_forward_datagram(SOCKET socket,
                               const char* data,
                               std::size_t size,
                               const sockaddr* peer_address,
                               socklen_t peer_len) {
    return sendto_bounded(socket, data, size, peer_address, peer_len);
}

void shutdown_port_forward_send(SOCKET socket) {
    shutdown_socket_send(socket);
}

std::string printable_port_forward_endpoint(const sockaddr* address, socklen_t address_len) {
    return numeric_socket_address(address, address_len);
}

std::string socket_local_endpoint(SOCKET socket) {
    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    socklen_t address_len = sizeof(address);
    if (socket_name(socket, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        throw PortForwardError(400, "port_bind_failed", socket_error_message("getsockname"));
    }
    return printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&address), address_len);
}

SOCKET bind_port_forward_socket(const std::string& endpoint, const std::string& protocol) {
    const std::vector<SocketAddress> addresses = resolve_endpoint(endpoint, protocol, true, "invalid_endpoint");
    SOCKET bound_socket = INVALID_SOCKET;

    for (std::size_t i = 0; i < addresses.size(); ++i) {
        const SocketAddress& current = addresses[i];
        bound_socket = create_socket_cloexec(current.family, current.socktype, current.protocol);
        if (bound_socket == INVALID_SOCKET) {
            continue;
        }

        (void)set_socket_reuseaddr(bound_socket);
#ifndef REMOTE_EXEC_CPP_WINSOCK1
        if (current.family == AF_INET6) {
            (void)set_socket_ipv6_only(bound_socket);
        }
#endif

        if (bind_socket(bound_socket, current.sockaddr_ptr(), current.address_len) == 0) {
            break;
        }

        close_socket(bound_socket);
        bound_socket = INVALID_SOCKET;
    }

    if (bound_socket == INVALID_SOCKET) {
        throw PortForwardError(400, "port_bind_failed", socket_error_message("bind"));
    }

    if (protocol == "tcp" && listen_socket(bound_socket, SOMAXCONN) != 0) {
        const std::string message = socket_error_message("listen");
        close_socket(bound_socket);
        throw PortForwardError(400, "port_bind_failed", message);
    }

    return bound_socket;
}

SOCKET connect_port_forward_socket(const std::string& endpoint, const std::string& protocol, unsigned long timeout_ms) {
    const std::vector<SocketAddress> addresses = resolve_endpoint(endpoint, protocol, false, "invalid_endpoint");
    SOCKET connected_socket = INVALID_SOCKET;

    for (std::size_t i = 0; i < addresses.size(); ++i) {
        const SocketAddress& current = addresses[i];
        connected_socket = create_socket_cloexec(current.family, current.socktype, current.protocol);
        if (connected_socket == INVALID_SOCKET) {
            continue;
        }

        bool connected = false;
        try {
            if (protocol == "tcp") {
                connected = tcp_connect_with_timeout(
                    connected_socket, current.sockaddr_ptr(), current.address_len, timeout_ms);
            } else {
                connected = connect_socket(connected_socket, current.sockaddr_ptr(), current.address_len) == 0;
            }
        } catch (...) {
            close_socket(connected_socket);
            throw;
        }
        if (connected) {
            return connected_socket;
        }

        close_socket(connected_socket);
        connected_socket = INVALID_SOCKET;
    }

    if (connected_socket == INVALID_SOCKET) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("connect"));
    }

    return connected_socket;
}

void send_all_socket(SOCKET socket, const std::string& data) {
    std::size_t offset = 0;
    while (offset < data.size()) {
        const int sent = send_bounded(socket, data.data() + offset, data.size() - offset, 0);
        if (sent <= 0) {
            throw PortForwardError(400, "port_write_failed", socket_error_message("send"));
        }
        offset += static_cast<std::size_t>(sent);
    }
}

SocketAddress parse_port_forward_peer(const std::string& peer) {
    const std::vector<SocketAddress> addresses = resolve_endpoint(peer, "udp", false, "invalid_endpoint");
    if (addresses.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "unable to resolve UDP peer `" + peer + "`");
    }
    return addresses.front();
}
