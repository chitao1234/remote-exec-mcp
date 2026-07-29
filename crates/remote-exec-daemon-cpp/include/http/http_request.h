#pragma once

#include <string>

#include "http/http_helpers.h"

class HttpParseError : public HttpRequestError {
public:
    explicit HttpParseError(const std::string& message) : HttpRequestError(message) {}
};

HttpRequest parse_http_request(const std::string& raw);
HttpRequest parse_http_request_head(const std::string& raw_headers);
