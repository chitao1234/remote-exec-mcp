#include <stdexcept>
#include <string>

#include "server_transport.h"
#include "server_transport_internal.h"
#include "win32_error.h"

namespace {

std::string socket_error_message_from_code(const std::string& operation, int error) {
    return error_message_from_code(operation.c_str(), static_cast<unsigned long>(error));
}

void throw_socket_option_error(const std::string& option, int error) {
    throw std::runtime_error(socket_error_message_from_code("setsockopt(" + option + ")", error));
}

} // namespace

void close_socket(SOCKET socket) {
    closesocket(socket);
}

void shutdown_socket(SOCKET socket) {
    shutdown(socket, SD_BOTH);
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

SOCKET accept_client(SOCKET listener) {
    return accept(listener, nullptr, nullptr);
}
