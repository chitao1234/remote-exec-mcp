#pragma once

#include <cstdint>
#include <stdexcept>
#include <string>

#include "json.hpp"

// Single JSON boundary for request parsing. The underlying implementation is
// deliberately kept in one place so it can be replaced without touching route
// handlers.
using Json = nlohmann::json;

class JsonRequestError : public std::runtime_error {
public:
    explicit JsonRequestError(const std::string& message) : std::runtime_error(message) {}
};

const std::size_t JSON_MAX_DEPTH = 32;

// Parses a request body and rejects non-object roots. Throws JsonRequestError
// for malformed input, excessive nesting, or a non-object root.
Json parse_request_json_object(const std::string& text);
