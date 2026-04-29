#pragma once

#include <cstddef>
#include <stdexcept>
#include <string>

#ifdef _WIN32
#include <winsock2.h>
#else
typedef int SOCKET;
const int INVALID_SOCKET = -1;
#endif

#include "config.h"

class BadHttpRequest : public std::runtime_error {
public:
    explicit BadHttpRequest(const std::string& message) : std::runtime_error(message) {}
};

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

std::string read_http_request(
    SOCKET client,
    std::size_t max_header_bytes,
    std::size_t max_body_bytes
);
void send_all(SOCKET client, const std::string& data);
SOCKET create_listener(const DaemonConfig& config);
SOCKET accept_client(SOCKET listener);
