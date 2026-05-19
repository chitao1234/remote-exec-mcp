#include "platform/socket.h"

#include <climits>

#ifdef _WIN32
#include <winsock2.h>
#else
#include <cerrno>
#include <sys/socket.h>

#include "platform/posix_eintr.h"
#endif

std::size_t bounded_socket_io_size(std::size_t remaining) {
    const std::size_t max_chunk = static_cast<std::size_t>(INT_MAX);
    return remaining > max_chunk ? max_chunk : remaining;
}

int recv_bounded(SOCKET client, char* data, std::size_t remaining, int flags) {
#ifdef _WIN32
    return recv(client, data, static_cast<int>(bounded_socket_io_size(remaining)), flags);
#else
    return posix_eintr::retry<int>(
        [&]() { return recv(client, data, static_cast<int>(bounded_socket_io_size(remaining)), flags); });
#endif
}

int send_bounded(SOCKET client, const char* data, std::size_t remaining, int flags) {
#ifdef _WIN32
    return send(client, data, static_cast<int>(bounded_socket_io_size(remaining)), flags);
#else
    return posix_eintr::retry<int>(
        [&]() { return send(client, data, static_cast<int>(bounded_socket_io_size(remaining)), flags); });
#endif
}

int recvfrom_bounded(SOCKET socket, char* data, std::size_t size, sockaddr* peer_address, socklen_t* peer_len) {
#ifdef _WIN32
    return recvfrom(socket, data, static_cast<int>(bounded_socket_io_size(size)), 0, peer_address, peer_len);
#else
    return posix_eintr::retry<int>([&]() {
        return recvfrom(socket,
                        data,
                        static_cast<int>(bounded_socket_io_size(size)),
                        0,
                        peer_address,
                        peer_len);
    });
#endif
}

int sendto_bounded(SOCKET socket, const char* data, std::size_t size, const sockaddr* peer_address, socklen_t peer_len) {
    if (size > static_cast<std::size_t>(INT_MAX)) {
#ifdef _WIN32
        WSASetLastError(WSAEMSGSIZE);
#else
        errno = EMSGSIZE;
#endif
        return -1;
    }
#ifdef _WIN32
    return sendto(socket, data, static_cast<int>(size), 0, peer_address, peer_len);
#else
    return posix_eintr::retry<int>(
        [&]() { return sendto(socket, data, static_cast<int>(size), 0, peer_address, peer_len); });
#endif
}

int connect_socket(SOCKET socket, const sockaddr* address, socklen_t address_len) {
#ifdef _WIN32
    return connect(socket, address, static_cast<int>(address_len));
#else
    return posix_eintr::retry<int>([&]() { return connect(socket, address, address_len); });
#endif
}

SOCKET accept_socket(SOCKET listener, sockaddr* peer_address, socklen_t* peer_len) {
#ifdef _WIN32
    return accept(listener, peer_address, peer_len);
#else
    return posix_eintr::retry<int>([&]() { return accept(listener, peer_address, peer_len); });
#endif
}

SOCKET accept_socket_cloexec(SOCKET listener, sockaddr* peer_address, socklen_t* peer_len) {
    SOCKET client = accept_socket(listener, peer_address, peer_len);
    if (client == INVALID_SOCKET) {
        return client;
    }
    if (set_socket_cloexec(client)) {
        return client;
    }
    const int cloexec_error = last_socket_error();
    close_socket(client);
#ifdef _WIN32
    WSASetLastError(cloexec_error);
#else
    errno = cloexec_error;
#endif
    return INVALID_SOCKET;
}

UniqueSocket::UniqueSocket() : socket_(INVALID_SOCKET) {
}

UniqueSocket::UniqueSocket(SOCKET socket) : socket_(socket) {
}

UniqueSocket::~UniqueSocket() {
    reset();
}

UniqueSocket::UniqueSocket(UniqueSocket&& other) : socket_(other.release()) {
}

UniqueSocket& UniqueSocket::operator=(UniqueSocket&& other) {
    if (this != &other) {
        reset(other.release());
    }
    return *this;
}

SOCKET UniqueSocket::get() const {
    return socket_;
}

bool UniqueSocket::valid() const {
    return socket_ != INVALID_SOCKET;
}

SOCKET UniqueSocket::release() {
    const SOCKET released = socket_;
    socket_ = INVALID_SOCKET;
    return released;
}

void UniqueSocket::reset(SOCKET socket) {
    if (valid()) {
        close_socket(socket_);
    }
    socket_ = socket;
}
