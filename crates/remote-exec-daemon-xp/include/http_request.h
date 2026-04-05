#pragma once

#include <string>

#include "http_helpers.h"

HttpRequest parse_http_request(const std::string& raw);
