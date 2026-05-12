#pragma once

#include <cassert>
#include <cstring>
#include <utility>

#ifdef _WIN32
#include <winsock2.h>
#else
#include <sys/socket.h>
#endif

#include "server_transport.h"

struct ConnectedSocketPair {
    ConnectedSocketPair(UniqueSocket first_socket, UniqueSocket second_socket)
        : first(std::move(first_socket)), second(std::move(second_socket)) {}

    UniqueSocket first;
    UniqueSocket second;
};

inline NetworkSession& test_network_session() {
    static NetworkSession session;
    return session;
}

inline ConnectedSocketPair make_connected_socket_pair() {
    test_network_session();

#ifdef _WIN32
    UniqueSocket listener(socket(AF_INET, SOCK_STREAM, IPPROTO_TCP));
    assert(listener.valid());

    sockaddr_in address;
    std::memset(&address, 0, sizeof(address));
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = 0;

    assert(bind(listener.get(), reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);
    assert(listen(listener.get(), 1) == 0);

    int address_len = sizeof(address);
    assert(getsockname(listener.get(), reinterpret_cast<sockaddr*>(&address), &address_len) == 0);

    UniqueSocket client(socket(AF_INET, SOCK_STREAM, IPPROTO_TCP));
    assert(client.valid());
    assert(connect(client.get(), reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);

    const SOCKET accepted_socket = accept(listener.get(), NULL, NULL);
    assert(accepted_socket != INVALID_SOCKET);
    return ConnectedSocketPair(UniqueSocket(accepted_socket), std::move(client));
#else
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);
    return ConnectedSocketPair(UniqueSocket(sockets[0]), UniqueSocket(sockets[1]));
#endif
}
