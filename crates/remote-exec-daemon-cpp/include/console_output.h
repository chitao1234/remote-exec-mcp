#pragma once

#include <string>

#include <windows.h>

std::string read_available_console_output(HANDLE pipe, std::string* carry);
std::string flush_console_output_carry(std::string* carry);
