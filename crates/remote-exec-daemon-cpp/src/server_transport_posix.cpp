#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <netdb.h>
#include <signal.h>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#include "server_transport.h"
#include "server_transport_internal.h"

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
    const int flags = fcntl(socket, F_GETFD, 0);
    if (flags < 0) {
        return false;
    }
    return fcntl(socket, F_SETFD, flags | FD_CLOEXEC) == 0;
}

} // namespace

void close_socket(SOCKET socket) {
    close(socket);
}

void shutdown_socket(SOCKET socket) {
    shutdown(socket, SHUT_RDWR);
}

SOCKET create_socket_cloexec(int family, int type, int protocol) {
    SOCKET created = INVALID_SOCKET;
#ifdef SOCK_CLOEXEC
    created = socket(family, type | SOCK_CLOEXEC, protocol);
    if (created != INVALID_SOCKET) {
        return created;
    }
    if (errno != EINVAL) {
        return INVALID_SOCKET;
    }
#endif
    created = socket(family, type, protocol);
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
    if (setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, &value, sizeof(value)) != 0) {
        throw_socket_option_error("SO_RCVTIMEO", errno);
    }
    if (setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, &value, sizeof(value)) != 0) {
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

SOCKET accept_client(SOCKET listener) {
    SOCKET client = accept(listener, nullptr, nullptr);
    if (client == INVALID_SOCKET) {
        return client;
    }
    if (set_socket_cloexec_flag(client)) {
        return client;
    }
    const int cloexec_error = errno;
    close_socket(client);
    errno = cloexec_error;
    return INVALID_SOCKET;
}
