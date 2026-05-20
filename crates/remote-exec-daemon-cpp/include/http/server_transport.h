#pragma once

#include <cstddef>
#include <map>
#include <stdexcept>
#include <string>

#include "core/config.h"
#include "http/http_codec.h"
#include "http/http_helpers.h"
#include "platform/socket.h"

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

struct HttpRequestHead {
    std::string raw_headers;
    std::string initial_body;
};

class HttpRequestBodyStream {
public:
    HttpRequestBodyStream(SOCKET client,
                          const std::string& initial_body,
                          const HttpRequestBodyFraming& framing,
                          std::size_t max_body_bytes);

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

class HttpChunkedResponseWriter {
public:
    explicit HttpChunkedResponseWriter(SOCKET client);

    void write_chunk(const char* data, std::size_t size);
    void write_chunk(const std::string& data);
    void finish();

private:
    SOCKET client_;
};

bool try_read_http_request_head(SOCKET client, std::size_t max_header_bytes, HttpRequestHead* head);
HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes);
std::string read_request_body_to_string(HttpRequestBodyStream* body);
void send_all(SOCKET client, const std::string& data);
void send_all_bytes(SOCKET client, const char* data, std::size_t size);
void send_http_response(SOCKET client, const HttpResponse& response);
void send_http_response_head(SOCKET client, const HttpResponse& response);
void send_http_upgrade_response(SOCKET client,
                                const std::string& upgrade_token,
                                const std::map<std::string, std::string>& headers);
SOCKET create_listener(const DaemonConfig& config);
SOCKET accept_client(SOCKET listener);
