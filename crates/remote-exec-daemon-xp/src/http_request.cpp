#include <sstream>
#include <stdexcept>
#include <string>

#include "http_request.h"
#include "text_utils.h"

HttpRequest parse_http_request(const std::string& raw) {
    const std::size_t header_end = raw.find("\r\n\r\n");
    if (header_end == std::string::npos) {
        throw std::runtime_error("invalid http request");
    }

    std::istringstream lines(raw.substr(0, header_end));
    std::string request_line;
    if (!std::getline(lines, request_line)) {
        throw std::runtime_error("missing request line");
    }
    if (!request_line.empty() && request_line[request_line.size() - 1] == '\r') {
        request_line.erase(request_line.size() - 1);
    }

    std::istringstream request_line_stream(request_line);
    HttpRequest request;
    request_line_stream >> request.method >> request.path;
    if (request.method.empty() || request.path.empty()) {
        throw std::runtime_error("invalid request line");
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

    request.body = raw.substr(header_end + 4);
    return request;
}
