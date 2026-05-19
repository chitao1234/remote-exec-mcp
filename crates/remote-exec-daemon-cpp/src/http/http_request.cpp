#include <cctype>
#include <limits>
#include <map>
#include <sstream>
#include <stdexcept>
#include <string>

#include "http_codec.h"
#include "http_request.h"
#include "text_utils.h"

namespace {

void validate_token(const std::string& value, const std::string& error_message) {
    if (value.empty()) {
        throw HttpParseError(error_message);
    }
    for (std::size_t i = 0; i < value.size(); ++i) {
        if (!is_http_token_char(value[i])) {
            throw HttpParseError(error_message);
        }
    }
}

void validate_request_target(const std::string& value) {
    if (value.empty() || value[0] != '/') {
        throw HttpParseError("invalid request target");
    }
    for (std::size_t i = 0; i < value.size(); ++i) {
        const unsigned char ch = static_cast<unsigned char>(value[i]);
        if (ch <= 32U || ch == 127U) {
            throw HttpParseError("invalid request target");
        }
    }
}

void validate_http_version(const std::string& value) {
    if (value != "HTTP/1.1") {
        throw HttpParseError("unsupported http version");
    }
}

} // namespace

HttpRequest parse_http_request_head(const std::string& raw_headers) {
    const std::size_t header_end = raw_headers.find("\r\n\r\n");
    const std::string header_block = header_end == std::string::npos ? raw_headers : raw_headers.substr(0, header_end);
    std::istringstream lines(header_block);
    std::string request_line;
    if (!std::getline(lines, request_line)) {
        throw HttpParseError("missing request line");
    }
    if (!request_line.empty() && request_line[request_line.size() - 1] == '\r') {
        request_line.erase(request_line.size() - 1);
    }

    std::istringstream request_line_stream(request_line);
    HttpRequest request;
    std::string version;
    std::string extra;
    request_line_stream >> request.method >> request.path >> version;
    if (request.method.empty() || request.path.empty() || version.empty() || (request_line_stream >> extra)) {
        throw HttpParseError("invalid request line");
    }
    validate_token(request.method, "invalid request method");
    validate_request_target(request.path);
    validate_http_version(version);

    std::string header_line;
    while (std::getline(lines, header_line)) {
        if (!header_line.empty() && header_line[header_line.size() - 1] == '\r') {
            header_line.erase(header_line.size() - 1);
        }
        if (header_line.empty()) {
            continue;
        }

        try {
            parse_http_header_line(header_line, &request.headers);
        } catch (const HttpProtocolError& ex) {
            throw HttpParseError(ex.what());
        }
    }

    return request;
}

HttpRequest parse_http_request(const std::string& raw) {
    const std::size_t header_end = raw.find("\r\n\r\n");
    if (header_end == std::string::npos) {
        throw HttpParseError("invalid http request");
    }

    HttpRequest request = parse_http_request_head(raw.substr(0, header_end));
    const std::string raw_body = raw.substr(header_end + 4U);

    try {
        const HttpRequestBodyFraming framing = request_body_framing_from_headers(request.headers);
        if (framing.chunked) {
            request.body = decode_http_chunked_body(raw_body);
        } else {
            if (framing.has_content_length && raw_body.size() != framing.content_length) {
                throw HttpParseError("Content-Length does not match body size");
            }
            request.body = raw_body;
        }
    } catch (const HttpProtocolError& ex) {
        throw HttpParseError(ex.what());
    }

    return request;
}
