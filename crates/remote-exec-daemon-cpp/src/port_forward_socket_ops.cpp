#include "port_forward_socket_ops.h"

#include <climits>
#include <cstring>
#include <sstream>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#else
#include <arpa/inet.h>
#include <cerrno>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "port_forward_endpoint.h"
#include "port_forward_error.h"
#include "win32_error.h"

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

addrinfo*
resolve_endpoint(const std::string& endpoint, const std::string& protocol, int flags, const char* error_code) {
    const ParsedPortForwardEndpoint parsed = endpoint_to_host_port(endpoint);

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = protocol_to_socktype(protocol);
    hints.ai_protocol = protocol_to_ipproto(protocol);
    hints.ai_flags = flags;

    addrinfo* result = nullptr;
    const int status = getaddrinfo(parsed.host.c_str(), parsed.port.c_str(), &hints, &result);
    if (status != 0 || result == nullptr) {
        const std::string operation = "resolving endpoint `" + endpoint + "`";
#ifdef _WIN32
        const std::string message = error_message_from_code(operation.c_str(), static_cast<unsigned long>(status));
#else
        std::ostringstream message;
        message << operation << " failed";
        message << ": " << gai_strerror(status);
        const std::string message_text = message.str();
#endif
#ifdef _WIN32
        throw PortForwardError(400, error_code, message);
#else
        throw PortForwardError(400, error_code, message_text);
#endif
    }
    return result;
}

bool connect_in_progress_error(int error) {
#ifdef _WIN32
    return error == WSAEWOULDBLOCK || error == WSAEINPROGRESS;
#else
    return error == EINPROGRESS;
#endif
}

void set_socket_nonblocking(SOCKET socket, bool enabled) {
#ifdef _WIN32
    u_long mode = enabled ? 1UL : 0UL;
    if (ioctlsocket(socket, FIONBIO, &mode) != 0) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("ioctlsocket"));
    }
#else
    const int flags = fcntl(socket, F_GETFL, 0);
    if (flags < 0) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("fcntl"));
    }
    const int updated = enabled ? (flags | O_NONBLOCK) : (flags & ~O_NONBLOCK);
    if (fcntl(socket, F_SETFL, updated) != 0) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("fcntl"));
    }
#endif
}

bool wait_for_connect(SOCKET socket, unsigned long timeout_ms) {
#ifdef _WIN32
    fd_set writefds;
    FD_ZERO(&writefds);
    FD_SET(socket, &writefds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int selected = select(0, nullptr, &writefds, nullptr, &timeout);
#else
    struct pollfd descriptor;
    descriptor.fd = socket;
    descriptor.events = POLLOUT;
    descriptor.revents = 0;

    const int timeout = timeout_ms > static_cast<unsigned long>(INT_MAX) ? INT_MAX : static_cast<int>(timeout_ms);
    int selected;
    for (;;) {
        selected = poll(&descriptor, 1, timeout);
        if (selected >= 0 || errno != EINTR) {
            break;
        }
    }
#endif
    if (selected < 0) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("poll"));
    }
#ifdef _WIN32
    return selected > 0 && FD_ISSET(socket, &writefds);
#else
    return selected > 0 && (descriptor.revents & POLLOUT) != 0;
#endif
}

bool tcp_connect_with_timeout(SOCKET socket, const sockaddr* address, socklen_t address_len, unsigned long timeout_ms) {
    set_socket_nonblocking(socket, true);
    if (connect(socket, address, static_cast<int>(address_len)) == 0) {
        set_socket_nonblocking(socket, false);
        return true;
    }

    const int connect_error = last_socket_error();
    if (!connect_in_progress_error(connect_error)) {
        set_socket_nonblocking(socket, false);
        return false;
    }

    if (!wait_for_connect(socket, timeout_ms)) {
        set_socket_nonblocking(socket, false);
        throw PortForwardError(400, "port_connect_failed", "tcp connect timed out");
    }

    int socket_error = 0;
    socklen_t socket_error_len = static_cast<socklen_t>(sizeof(socket_error));
    if (getsockopt(socket, SOL_SOCKET, SO_ERROR, reinterpret_cast<char*>(&socket_error), &socket_error_len) != 0) {
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

std::string printable_port_forward_endpoint(const sockaddr* address, socklen_t address_len) {
    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    const int result = getnameinfo(
        address, address_len, host, sizeof(host), service, sizeof(service), NI_NUMERICHOST | NI_NUMERICSERV);
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

    for (addrinfo* current = result; current != nullptr; current = current->ai_next) {
        bound_socket = create_socket_cloexec(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (bound_socket == INVALID_SOCKET) {
            continue;
        }

        int yes = 1;
        setsockopt(bound_socket, SOL_SOCKET, SO_REUSEADDR, reinterpret_cast<const char*>(&yes), sizeof(yes));

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

SOCKET connect_port_forward_socket(const std::string& endpoint, const std::string& protocol, unsigned long timeout_ms) {
    addrinfo* result = resolve_endpoint(endpoint, protocol, 0, "invalid_endpoint");
    SOCKET connected_socket = INVALID_SOCKET;

    for (addrinfo* current = result; current != nullptr; current = current->ai_next) {
        connected_socket = create_socket_cloexec(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (connected_socket == INVALID_SOCKET) {
            continue;
        }

        bool connected = false;
        try {
            if (protocol == "tcp") {
                connected = tcp_connect_with_timeout(
                    connected_socket, current->ai_addr, static_cast<socklen_t>(current->ai_addrlen), timeout_ms);
            } else {
                connected = connect(connected_socket, current->ai_addr, static_cast<int>(current->ai_addrlen)) == 0;
            }
        } catch (...) {
            close_socket(connected_socket);
            freeaddrinfo(result);
            throw;
        }
        if (connected) {
            freeaddrinfo(result);
            return connected_socket;
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
        const int sent = send(socket, data.data() + offset, static_cast<int>(data.size() - offset), 0);
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
    if (result != nullptr) {
        std::memcpy(&address, result->ai_addr, result->ai_addrlen);
        *peer_len = static_cast<socklen_t>(result->ai_addrlen);
    }
    freeaddrinfo(result);
    return address;
}
