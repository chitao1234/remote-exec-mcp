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

struct HttpRequestHead {
    std::string raw_headers;
    std::string initial_body;
};

struct HttpRequestBodyFraming {
    HttpRequestBodyFraming();

    bool has_content_length;
    std::size_t content_length;
    bool chunked;
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
std::string socket_error_message(const std::string& operation);
void close_socket(SOCKET socket);
void shutdown_socket(SOCKET socket);
HttpRequestBodyFraming request_body_framing_from_headers(const std::string& header_block);
HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes);
std::string read_http_request(
    SOCKET client,
    std::size_t max_header_bytes,
    std::size_t max_body_bytes
);
void send_all(SOCKET client, const std::string& data);
void send_all_bytes(SOCKET client, const char* data, std::size_t size);
SOCKET create_listener(const DaemonConfig& config);
SOCKET accept_client(SOCKET listener);
