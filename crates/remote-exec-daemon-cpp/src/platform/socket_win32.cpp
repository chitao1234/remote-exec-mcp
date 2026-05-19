#ifdef _WIN32

#include "platform/socket.h"

#include <cstring>
#include <stdexcept>
#include <string>
#include <ws2tcpip.h>

#include "platform/win32_error.h"

namespace {

std::string socket_error_message_from_code(const std::string& operation, int error) {
    return error_message_from_code(operation.c_str(), static_cast<unsigned long>(error));
}

void throw_socket_option_error(const std::string& option, int error) {
    throw std::runtime_error(socket_error_message_from_code("setsockopt(" + option + ")", error));
}

int get_socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len) {
    return getsockname(socket, address, address_len);
}

} // namespace

void close_socket(SOCKET socket) {
    closesocket(socket);
}

void shutdown_socket(SOCKET socket) {
    shutdown(socket, SD_BOTH);
}

void shutdown_socket_send(SOCKET socket) {
    shutdown(socket, SD_SEND);
}

bool set_socket_cloexec(SOCKET socket) {
    (void)socket;
    return true;
}

SOCKET create_socket_cloexec(int family, int type, int protocol) {
    return socket(family, type, protocol);
}

void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms) {
    const DWORD value = static_cast<DWORD>(timeout_ms);
    if (setsockopt(socket, SOL_SOCKET, SO_RCVTIMEO, reinterpret_cast<const char*>(&value), sizeof(value)) != 0) {
        throw_socket_option_error("SO_RCVTIMEO", WSAGetLastError());
    }
    if (setsockopt(socket, SOL_SOCKET, SO_SNDTIMEO, reinterpret_cast<const char*>(&value), sizeof(value)) != 0) {
        throw_socket_option_error("SO_SNDTIMEO", WSAGetLastError());
    }
}

int last_socket_error() {
    return WSAGetLastError();
}

std::string socket_error_message(const std::string& operation) {
    return socket_error_message_from_code(operation, last_socket_error());
}

bool would_block_error(int error) {
    return error == WSAEWOULDBLOCK;
}

bool peer_disconnected_send_error(int error) {
    return error == WSAECONNABORTED || error == WSAECONNRESET || error == WSAESHUTDOWN;
}

bool receive_timeout_error(int error) {
    return error == WSAETIMEDOUT || error == WSAEWOULDBLOCK;
}

NetworkSession::NetworkSession() {
    WSADATA wsa_data;
    if (WSAStartup(MAKEWORD(2, 2), &wsa_data) != 0) {
        throw std::runtime_error("WSAStartup failed");
    }
}

NetworkSession::~NetworkSession() {
    WSACleanup();
}

int wait_socket_readable_or_wakeup(SOCKET socket, SOCKET wakeup_fd, unsigned long timeout_ms) {
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(socket, &readfds);
    FD_SET(wakeup_fd, &readfds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(0, &readfds, nullptr, nullptr, &timeout);
    if (ready <= 0) {
        return ready;
    }
    if (FD_ISSET(wakeup_fd, &readfds)) {
        return -1;
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
