#pragma once

#include <string>

std::string last_error_message(const char* prefix);
std::string error_message_from_code(const char* prefix, unsigned long error);
