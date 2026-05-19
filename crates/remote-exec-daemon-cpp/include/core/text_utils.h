#pragma once

#include <string>

bool is_http_token_char(char ch);
std::string trim_ascii(const std::string& raw);
std::string lowercase_ascii(std::string value);
