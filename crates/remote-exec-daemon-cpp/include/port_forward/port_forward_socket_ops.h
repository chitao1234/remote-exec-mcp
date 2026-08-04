#pragma once

#include <cstddef>
#include <string>

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <sys/socket.h>
#include <sys/types.h>
#endif

#include "core/config.h"
#include "platform/socket.h"

SOCKET bind_port_forward_socket(const std::string& endpoint, const std::string& protocol);
void set_socket_nonblocking(SOCKET socket, bool enabled);
SOCKET connect_port_forward_socket(
    const std::string& endpoint,
    const std::string& protocol,
    unsigned long timeout_ms = DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS
);
SOCKET accept_port_forward_peer(SOCKET listener, sockaddr* peer_address, socklen_t* peer_len);
int recv_port_forward_datagram(
    SOCKET socket,
    char* data,
    std::size_t size,
    sockaddr* peer_address,
    socklen_t* peer_len
);
int send_port_forward_datagram(
    SOCKET socket,
    const char* data,
    std::size_t size,
    const sockaddr* peer_address,
    socklen_t peer_len
);
void shutdown_port_forward_send(SOCKET socket);
std::string printable_port_forward_endpoint(const sockaddr* address, socklen_t address_len);
std::string socket_local_endpoint(SOCKET socket);
void send_all_socket(SOCKET socket, const char* data, std::size_t size);
void send_all_socket(SOCKET socket, const std::string& data);
SocketAddress parse_port_forward_peer(const std::string& peer);
