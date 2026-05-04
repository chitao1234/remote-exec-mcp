#pragma once

#include <string>

#ifndef _WIN32
#include <sys/socket.h>
#include <sys/types.h>
#endif

#include "server_transport.h"

SOCKET bind_port_forward_socket(const std::string& endpoint, const std::string& protocol);
SOCKET connect_port_forward_socket(const std::string& endpoint, const std::string& protocol);
std::string printable_port_forward_endpoint(const sockaddr* address, socklen_t address_len);
std::string socket_local_endpoint(SOCKET socket);
void send_all_socket(SOCKET socket, const std::string& data);
sockaddr_storage parse_port_forward_peer(const std::string& peer, socklen_t* peer_len);
