#include "http/server_transport.h"
#include "platform/socket.h"

SOCKET accept_client(SOCKET listener) {
    return accept_socket_cloexec(listener, nullptr, nullptr);
}
