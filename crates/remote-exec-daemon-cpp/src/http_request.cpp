#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>

#include "http_request.h"
#include "text_utils.h"

namespace {

int hex_digit_value(char ch) {
    if (ch >= '0' && ch <= '9') {
        return ch - '0';
    }
    if (ch >= 'a' && ch <= 'f') {
        return ch - 'a' + 10;
    }
    if (ch >= 'A' && ch <= 'F') {
        return ch - 'A' + 10;
    }
    return -1;
}

std::size_t parse_chunk_size_line(const std::string& line) {
    const std::size_t extension = line.find(';');
    const std::string size_text =
        trim_ascii(extension == std::string::npos ? line : line.substr(0, extension));
    if (size_text.empty()) {
        throw HttpParseError("invalid chunk size");
    }

    std::size_t value = 0;
    for (std::size_t i = 0; i < size_text.size(); ++i) {
        const int digit = hex_digit_value(size_text[i]);
        if (digit < 0) {
            throw HttpParseError("invalid chunk size");
        }
        const std::size_t chunk_digit = static_cast<std::size_t>(digit);
        if (value > (std::numeric_limits<std::size_t>::max() - chunk_digit) / 16U) {
            throw HttpParseError("chunk size is too large");
        }
        value = value * 16U + chunk_digit;
    }
    return value;
}

std::string decode_chunked_body(const std::string& body) {
    std::string decoded;
    std::size_t offset = 0;

    for (;;) {
        const std::size_t line_end = body.find("\r\n", offset);
        if (line_end == std::string::npos) {
            throw HttpParseError("incomplete chunked body");
        }

        const std::size_t chunk_size =
            parse_chunk_size_line(body.substr(offset, line_end - offset));
        offset = line_end + 2U;

        if (chunk_size == 0U) {
            if (body.size() >= offset + 2U && body.compare(offset, 2U, "\r\n") == 0) {
                return decoded;
            }
            if (body.find("\r\n\r\n", offset) != std::string::npos) {
                return decoded;
            }
            throw HttpParseError("incomplete chunked body");
        }

        if (chunk_size > body.size() - offset) {
            throw HttpParseError("incomplete chunked body");
        }
        const std::size_t chunk_end = offset + chunk_size;
        if (body.size() - chunk_end < 2U) {
            throw HttpParseError("incomplete chunked body");
        }
        if (body.compare(chunk_end, 2U, "\r\n") != 0) {
            throw HttpParseError("invalid chunked body");
        }

        decoded.append(body, offset, chunk_size);
        offset = chunk_end + 2U;
    }
}

}  // namespace

HttpRequest parse_http_request(const std::string& raw) {
    const std::size_t header_end = raw.find("\r\n\r\n");
    if (header_end == std::string::npos) {
        throw HttpParseError("invalid http request");
    }

    std::istringstream lines(raw.substr(0, header_end));
    std::string request_line;
    if (!std::getline(lines, request_line)) {
        throw HttpParseError("missing request line");
    }
    if (!request_line.empty() && request_line[request_line.size() - 1] == '\r') {
        request_line.erase(request_line.size() - 1);
    }

    std::istringstream request_line_stream(request_line);
    HttpRequest request;
    request_line_stream >> request.method >> request.path;
    if (request.method.empty() || request.path.empty()) {
        throw HttpParseError("invalid request line");
    }

    std::string header_line;
    while (std::getline(lines, header_line)) {
        if (!header_line.empty() && header_line[header_line.size() - 1] == '\r') {
            header_line.erase(header_line.size() - 1);
        }
        if (header_line.empty()) {
            continue;
        }

        const std::size_t colon = header_line.find(':');
        if (colon == std::string::npos) {
            continue;
        }

        request.headers[lowercase_ascii(trim_ascii(header_line.substr(0, colon)))] =
            trim_ascii(header_line.substr(colon + 1));
    }

    const std::string transfer_encoding =
        lowercase_ascii(trim_ascii(request.header("transfer-encoding")));
    if (!transfer_encoding.empty()) {
        if (transfer_encoding != "chunked") {
            throw HttpParseError("unsupported transfer encoding");
        }
        if (!request.header("content-length").empty()) {
            throw HttpParseError("chunked request cannot include Content-Length");
        }
        request.body = decode_chunked_body(raw.substr(header_end + 4U));
    } else {
        request.body = raw.substr(header_end + 4U);
    }
    return request;
}
