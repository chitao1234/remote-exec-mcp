#pragma once

#include <stdexcept>
#include <string>

#include "http_helpers.h"

class HttpParseError : public std::runtime_error {
public:
    explicit HttpParseError(const std::string& message) : std::runtime_error(message) {}
};

HttpRequest parse_http_request(const std::string& raw);
