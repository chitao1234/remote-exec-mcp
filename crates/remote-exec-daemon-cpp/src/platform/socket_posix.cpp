#ifndef _WIN32

#include "platform/socket.h"

#include <arpa/inet.h>
#include <cerrno>
#include <cstring>
#include <netinet/in.h>
#include <poll.h>
#include <signal.h>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"

namespace {

std::string socket_error_message_from_code(const std::string& operation, int error) {
    std::ostringstream out;
    out << operation << " failed";
    out << ": " << std::strerror(error);
    return out.str();
}

void throw_socket_option_error(const std::string& option, int error) {
    throw std::runtime_error(socket_error_message_from_code("setsockopt(" + option + ")", error));
}

bool set_socket_cloexec_flag(SOCKET socket) {
    return posix_fd::set_cloexec(socket);
}

int get_socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len) {
    return posix_eintr::retry<int>([&]() { return getsockname(socket, address, address_len); });
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
#ifdef SOCK_CLOEXEC
    created = posix_eintr::retry<int>([&]() { return socket(family, type | SOCK_CLOEXEC, protocol); });
    if (created != INVALID_SOCKET) {
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

NetworkSession::NetworkSession() {
    signal(SIGPIPE, SIG_IGN);
}

NetworkSession::~NetworkSession() {
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
