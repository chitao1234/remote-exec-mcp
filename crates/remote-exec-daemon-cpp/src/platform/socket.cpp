#include "platform/socket.h"

#include <climits>

#ifdef _WIN32
#include <winsock2.h>
#else
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
