#ifndef _WIN32

#include "platform/socket.h"

#include <arpa/inet.h>
#include <cerrno>
#include <cstring>
#include <netinet/in.h>
#include <poll.h>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#include "platform/platform.h"
#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"
#include "platform/posix_signal.h"
#include "remote_exec_cpp_config.h"

namespace {

std::string socket_error_message_from_code(const std::string& operation, int error) {
    std::ostringstream out;
    out << operation << " failed";
    out << ": " << errno_error::message_from_errno(error);
    return out.str();
}

void throw_socket_option_error(const std::string& option, int error) {
    throw std::runtime_error(socket_error_message_from_code("setsockopt(" + option + ")", error));
}

bool set_socket_cloexec_flag(SOCKET socket) {
    return posix_fd::set_cloexec(socket);
}

void apply_socket_sigpipe_policy(SOCKET socket) {
#if !REMOTE_EXEC_CPP_HAVE_MSG_NOSIGNAL && REMOTE_EXEC_CPP_HAVE_SO_NOSIGPIPE
    int yes = 1;
    (void)posix_eintr::retry<int>([&]() { return setsockopt(socket, SOL_SOCKET, SO_NOSIGPIPE, &yes, sizeof(yes)); });
#else
    (void)socket;
#endif
}

int get_socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len) {
    return posix_eintr::retry<int>([&]() { return getsockname(socket, address, address_len); });
}

std::string gai_error_message(const std::string& operation, int status) {
    std::ostringstream out;
    out << operation << " failed";
    out << ": " << gai_strerror(status);
    return out.str();
}

} // namespace

void close_socket(SOCKET socket) {
    posix_fd::close_ignoring_errors(socket);
}

void shutdown_socket(SOCKET socket) {
    (void)posix_eintr::retry<int>([&]() { return shutdown(socket, SHUT_RDWR); });
}

void shutdown_socket_send(SOCKET socket) {
    (void)posix_eintr::retry<int>([&]() { return shutdown(socket, SHUT_WR); });
}

bool set_socket_cloexec(SOCKET socket) {
    return set_socket_cloexec_flag(socket);
}

SOCKET create_socket_cloexec(int family, int type, int protocol) {
    SOCKET created = INVALID_SOCKET;
#if REMOTE_EXEC_CPP_HAVE_SOCK_CLOEXEC
    created = posix_eintr::retry<int>([&]() { return socket(family, type | SOCK_CLOEXEC, protocol); });
    if (created != INVALID_SOCKET) {
        apply_socket_sigpipe_policy(created);
        return created;
    }
    if (errno != EINVAL) {
        return INVALID_SOCKET;
    }
#endif
    created = posix_eintr::retry<int>([&]() { return socket(family, type, protocol); });
    if (created == INVALID_SOCKET) {
        return INVALID_SOCKET;
    }
    if (set_socket_cloexec_flag(created)) {
        apply_socket_sigpipe_policy(created);
        return created;
    }
    const int cloexec_error = errno;
    close_socket(created);
    errno = cloexec_error;
    return INVALID_SOCKET;
}

void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms) {
    timeval value;
    value.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    value.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    if (posix_eintr::retry<int>([&]() { return setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, &value, sizeof(value)); }) !=
        0) {
        throw_socket_option_error("SO_RCVTIMEO", errno);
    }
    if (posix_eintr::retry<int>([&]() { return setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, &value, sizeof(value)); }) !=
        0) {
        throw_socket_option_error("SO_SNDTIMEO", errno);
    }
}

int last_socket_error() {
    return errno;
}

std::string socket_error_message(const std::string& operation) {
    return socket_error_message_from_code(operation, last_socket_error());
}

bool would_block_error(int error) {
    return error == EAGAIN || error == EWOULDBLOCK;
}

bool peer_disconnected_send_error(int error) {
    return error == EPIPE || error == ECONNRESET || error == ENOTCONN;
}

bool receive_timeout_error(int error) {
    return error == EAGAIN || error == EWOULDBLOCK;
}

bool connect_in_progress_socket_error(int error) {
    return error == EINPROGRESS || error == EINTR;
}

NetworkSession::NetworkSession() {
    if (posix_signal::ignore_signal(SIGPIPE) != 0) {
        throw std::runtime_error(socket_error_message_from_code("sigaction(SIGPIPE)", errno));
    }
}

NetworkSession::~NetworkSession() {
}

bool resolve_socket_addresses(const char* node,
                              const char* service,
                              const SocketAddressQuery& query,
                              std::vector<SocketAddress>* addresses,
                              std::string* error) {
    addresses->clear();

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = query.family;
    hints.ai_socktype = query.socktype;
    hints.ai_protocol = query.protocol;
    hints.ai_flags = query.passive ? AI_PASSIVE : 0;

    addrinfo* result = nullptr;
    const int status = posix_eintr::retry_eai_system([&]() { return getaddrinfo(node, service, &hints, &result); });
    if (status != 0 || result == nullptr) {
        if (error != nullptr) {
            *error = gai_error_message("getaddrinfo", status);
        }
        return false;
    }

    for (addrinfo* current = result; current != nullptr; current = current->ai_next) {
        if (current->ai_addrlen > sizeof(SocketAddress().address)) {
            continue;
        }
        SocketAddress address;
        address.family = current->ai_family;
        address.socktype = current->ai_socktype;
        address.protocol = current->ai_protocol;
        address.address_len = static_cast<socklen_t>(current->ai_addrlen);
        std::memcpy(&address.address, current->ai_addr, current->ai_addrlen);
        addresses->push_back(address);
    }
    freeaddrinfo(result);

    if (addresses->empty()) {
        if (error != nullptr) {
            *error = "getaddrinfo failed: no usable socket addresses";
        }
        return false;
    }
    return true;
}

std::string numeric_socket_address(const sockaddr* address, socklen_t address_len) {
    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    const int result = posix_eintr::retry_eai_system([&]() {
        return getnameinfo(address,
                           address_len,
                           host,
                           static_cast<socklen_t>(sizeof(host)),
                           service,
                           static_cast<socklen_t>(sizeof(service)),
                           NI_NUMERICHOST | NI_NUMERICSERV);
    });
    if (result != 0) {
        return "unknown:0";
    }

    if (address->sa_family == AF_INET6) {
        return "[" + std::string(host) + "]:" + std::string(service);
    }
    return std::string(host) + ":" + std::string(service);
}

int wait_socket_readable_or_wakeup(SOCKET socket, SOCKET wakeup_fd, unsigned long timeout_ms) {
    struct pollfd fds[2];
    fds[0].fd = socket;
    fds[0].events = POLLIN;
    fds[0].revents = 0;
    fds[1].fd = wakeup_fd;
    fds[1].events = POLLIN;
    fds[1].revents = 0;

    const int ready = posix_eintr::poll_for_ms(fds, 2, timeout_ms);
    if (ready > 0) {
        if (fds[1].revents & (POLLIN | POLLHUP | POLLNVAL | POLLERR)) {
            return -1;
        }
        if (fds[0].revents & (POLLNVAL | POLLERR)) {
            return -1;
        }
    }
    return ready;
}

int wait_socket_readable(SOCKET socket, unsigned long timeout_ms) {
    struct pollfd descriptor;
    descriptor.fd = socket;
    descriptor.events = POLLIN;
    descriptor.revents = 0;

    const int ready = posix_eintr::poll_for_ms(&descriptor, 1, timeout_ms);
    if (ready > 0 && (descriptor.revents & (POLLNVAL | POLLERR)) != 0) {
        return -1;
    }
    return ready;
}

int wait_socket_writable(SOCKET socket, unsigned long timeout_ms) {
    struct pollfd descriptor;
    descriptor.fd = socket;
    descriptor.events = POLLOUT;
    descriptor.revents = 0;

    const int ready = posix_eintr::poll_for_ms(&descriptor, 1, timeout_ms);
    if (ready <= 0) {
        return ready;
    }
    if ((descriptor.revents & POLLNVAL) != 0) {
        return -1;
    }
    return (descriptor.revents & (POLLOUT | POLLERR | POLLHUP)) != 0 ? 1 : 0;
}

int socket_error_option(SOCKET socket, int* socket_error) {
    socklen_t socket_error_len = static_cast<socklen_t>(sizeof(*socket_error));
    return posix_eintr::retry<int>(
        [&]() { return getsockopt(socket, SOL_SOCKET, SO_ERROR, socket_error, &socket_error_len); });
}

int set_socket_reuseaddr(SOCKET socket) {
    int yes = 1;
    return posix_eintr::retry<int>([&]() { return setsockopt(socket, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes)); });
}

int set_socket_ipv6_only(SOCKET socket) {
    int yes = 1;
    return posix_eintr::retry<int>([&]() { return setsockopt(socket, IPPROTO_IPV6, IPV6_V6ONLY, &yes, sizeof(yes)); });
}

int bind_socket(SOCKET socket, const sockaddr* address, socklen_t address_len) {
    return posix_eintr::retry<int>([&]() { return bind(socket, address, address_len); });
}

int listen_socket(SOCKET socket, int backlog) {
    return posix_eintr::retry<int>([&]() { return listen(socket, backlog); });
}

int socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len) {
    return get_socket_name(socket, address, address_len);
}

unsigned short socket_bound_port_or_zero(SOCKET socket) {
    if (socket == INVALID_SOCKET) {
        return 0;
    }

    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    socklen_t address_len = sizeof(address);
    if (get_socket_name(socket, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        return 0;
    }

    if (address.ss_family == AF_INET) {
        const sockaddr_in* ipv4 = reinterpret_cast<const sockaddr_in*>(&address);
        return ntohs(ipv4->sin_port);
    }
    if (address.ss_family == AF_INET6) {
        const sockaddr_in6* ipv6 = reinterpret_cast<const sockaddr_in6*>(&address);
        return ntohs(ipv6->sin6_port);
    }

    return 0;
}

#endif
