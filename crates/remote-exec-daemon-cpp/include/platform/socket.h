#pragma once

#include <cstddef>
#include <string>

#ifdef _WIN32
#include <winsock2.h>
#else
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
std::size_t bounded_socket_io_size(std::size_t remaining);
int recv_bounded(SOCKET client, char* data, std::size_t remaining, int flags);
int send_bounded(SOCKET client, const char* data, std::size_t remaining, int flags);
std::string socket_error_message(const std::string& operation);
void close_socket(SOCKET socket);
void shutdown_socket(SOCKET socket);
void shutdown_socket_send(SOCKET socket);
bool set_socket_cloexec(SOCKET socket);
SOCKET create_socket_cloexec(int family, int type, int protocol);
void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms);
int wait_socket_readable_or_wakeup(SOCKET socket, SOCKET wakeup_fd, unsigned long timeout_ms);
unsigned short socket_bound_port_or_zero(SOCKET socket);
