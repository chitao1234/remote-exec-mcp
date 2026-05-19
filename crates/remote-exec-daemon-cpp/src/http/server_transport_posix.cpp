#include <cerrno>
#include <sys/socket.h>

#include "platform/posix_eintr.h"
#include "platform/socket.h"
#include "http/server_transport.h"

SOCKET accept_client(SOCKET listener) {
    SOCKET client = posix_eintr::retry<int>([&]() { return accept(listener, nullptr, nullptr); });
    if (client == INVALID_SOCKET) {
        return client;
    }
    if (set_socket_cloexec(client)) {
        return client;
    }
    const int cloexec_error = errno;
    close_socket(client);
    errno = cloexec_error;
    return INVALID_SOCKET;
}
