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
#include "http_codec.h"

class BadHttpRequest : public std::runtime_error {
public:
    explicit BadHttpRequest(const std::string& message) : std::runtime_error(message) {}
};

class SocketSendError : public std::runtime_error {
public:
    SocketSendError(const std::string& message, bool peer_disconnected)
        : std::runtime_error(message), peer_disconnected_(peer_disconnected) {}

    bool peer_disconnected() const { return peer_disconnected_; }

private:
    bool peer_disconnected_;
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

struct HttpRequestHead {
    std::string raw_headers;
    std::string initial_body;
};

class HttpRequestBodyStream {
public:
    HttpRequestBodyStream(
        SOCKET client,
        const std::string& initial_body,
        const HttpRequestBodyFraming& framing,
        std::size_t max_body_bytes
    );

    std::size_t read(char* data, std::size_t max_size);

private:
    std::size_t read_content_length_body(char* data, std::size_t max_size);
    std::size_t read_chunked_body(char* data, std::size_t max_size);
    void ensure_raw_available(std::size_t size);
    void ensure_raw_line();
    void consume_raw(std::size_t size);
    void consume_chunk_trailers();

    SOCKET client_;
    std::string raw_;
    std::size_t raw_offset_;
    HttpRequestBodyFraming framing_;
    std::size_t decoded_size_;
    std::size_t max_body_bytes_;
    std::size_t remaining_content_length_;
    std::size_t remaining_chunk_size_;
    bool chunked_finished_;
};

int last_socket_error();
bool would_block_error(int error);
bool receive_timeout_error(int error);
std::size_t bounded_socket_io_size(std::size_t remaining);
int recv_bounded(SOCKET client, char* data, std::size_t remaining, int flags);
int send_bounded(SOCKET client, const char* data, std::size_t remaining, int flags);
std::string socket_error_message(const std::string& operation);
void close_socket(SOCKET socket);
void shutdown_socket(SOCKET socket);
void set_socket_timeout_ms(SOCKET socket, unsigned long timeout_ms);
bool try_read_http_request_head(
    SOCKET client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
);
HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes);
void send_all(SOCKET client, const std::string& data);
void send_all_bytes(SOCKET client, const char* data, std::size_t size);
SOCKET create_listener(const DaemonConfig& config);
SOCKET accept_client(SOCKET listener);
