#include "platform/socket.h"
#include "http/server_transport.h"

SOCKET accept_client(SOCKET listener) {
    return accept_socket_cloexec(listener, nullptr, nullptr);
}
