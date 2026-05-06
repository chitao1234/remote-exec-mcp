#pragma once

#include <cstddef>
#include <map>
#include <stdexcept>
#include <string>

class HttpProtocolError : public std::runtime_error {
public:
    explicit HttpProtocolError(const std::string& message)
        : std::runtime_error(message) {}
};

struct HttpRequestBodyFraming {
    HttpRequestBodyFraming();

    bool has_content_length;
    std::size_t content_length;
    bool chunked;
};

void parse_http_header_line(
    const std::string& header_line,
    std::map<std::string, std::string>* headers
);
HttpRequestBodyFraming request_body_framing_from_headers(
    const std::map<std::string, std::string>& headers
);
std::size_t parse_http_chunk_size_line(const std::string& line);
std::string decode_http_chunked_body(const std::string& body);
