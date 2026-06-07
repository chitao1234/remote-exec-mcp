#pragma once

#include <cstddef>
#include <string>
#include <vector>

#ifdef _WIN32
#include "platform/win32_socket_compat.h"
#else
#include <netdb.h>
#include <sys/socket.h>
typedef int SOCKET;
const int INVALID_SOCKET = -1;
#endif

class UniqueSocket {
public:
    UniqueSocket();
    explicit UniqueSocket(SOCKET socket);
    ~UniqueSocket();

    UniqueSocket(UniqueSocket&& other);
    UniqueSocket& operator=(UniqueSocket&& other);

    UniqueSocket(const UniqueSocket&) = delete;
    UniqueSocket& operator=(const UniqueSocket&) = delete;

    SOCKET get() const;
    bool valid() const;
    SOCKET release();
    void reset(SOCKET socket = INVALID_SOCKET);

private:
    SOCKET socket_;
};

struct SocketAddress {
    SocketAddress();

    sockaddr* sockaddr_ptr();
    const sockaddr* sockaddr_ptr() const;

    int family;
    int socktype;
    int protocol;
    sockaddr_storage address;
    socklen_t address_len;
};

struct SocketAddressQuery {
    SocketAddressQuery();

    int family;
    int socktype;
    int protocol;
    bool passive;
};

class NetworkSession {
public:
    NetworkSession();
    ~NetworkSession();

    NetworkSession(const NetworkSession&) = delete;
    NetworkSession& operator=(const NetworkSession&) = delete;
};

int last_socket_error();
bool would_block_error(int error);
bool peer_disconnected_send_error(int error);
bool receive_timeout_error(int error);
bool connect_in_progress_socket_error(int error);
std::size_t bounded_socket_io_size(std::size_t remaining);
int recv_bounded(SOCKET client, char* data, std::size_t remaining, int flags);
int send_bounded(SOCKET client, const char* data, std::size_t remaining, int flags);
int recvfrom_bounded(
    SOCKET socket,
    char* data,
    std::size_t size,
    sockaddr* peer_address,
    socklen_t* peer_len
);
int sendto_bounded(
    SOCKET socket,
    const char* data,
    std::size_t size,
    const sockaddr* peer_address,
    socklen_t peer_len
);
bool resolve_socket_addresses(
    const char* node,
    const char* service,
    const SocketAddressQuery& query,
    std::vector<SocketAddress>* addresses,
    std::string* error
);
std::string numeric_socket_address(const sockaddr* address, socklen_t address_len);
std::string socket_error_message(const std::string& operation);
void close_socket(SOCKET socket);
void shutdown_socket(SOCKET socket);
void shutdown_socket_send(SOCKET socket);
bool set_socket_cloexec(SOCKET socket);
SOCKET create_socket_cloexec(int family, int type, int protocol);
int connect_socket(SOCKET socket, const sockaddr* address, socklen_t address_len);
SOCKET accept_socket(SOCKET listener, sockaddr* peer_address, socklen_t* peer_len);
SOCKET accept_socket_cloexec(SOCKET listener, sockaddr* peer_address, socklen_t* peer_len);
void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms);
int wait_socket_readable_or_wakeup(SOCKET socket, SOCKET wakeup_fd, unsigned long timeout_ms);
int wait_socket_readable(SOCKET socket, unsigned long timeout_ms);
int wait_socket_writable(SOCKET socket, unsigned long timeout_ms);
int socket_error_option(SOCKET socket, int* socket_error);
int set_socket_reuseaddr(SOCKET socket);
int set_socket_ipv6_only(SOCKET socket);
int bind_socket(SOCKET socket, const sockaddr* address, socklen_t address_len);
int listen_socket(SOCKET socket, int backlog);
int socket_name(SOCKET socket, sockaddr* address, socklen_t* address_len);
unsigned short socket_bound_port_or_zero(SOCKET socket);
