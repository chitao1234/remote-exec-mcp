#pragma once

#include <string>

#include <windows.h>

std::string read_available_console_output(HANDLE pipe, std::string* carry);
std::string read_console_output(HANDLE pipe, bool block, bool* eof, std::string* carry);
std::string flush_console_output_carry(std::string* carry);

#ifdef REMOTE_EXEC_CPP_TESTING
std::string utf8_from_windows_wide_for_test(const std::wstring& wide);
std::string utf8_from_windows_code_page_for_test(unsigned int code_page, const std::string& raw);
std::string decode_console_output_for_test(unsigned int primary_code_page,
                                           unsigned int fallback_code_page,
                                           std::string* carry,
                                           const std::string& raw_chunk,
                                           bool flush);
#endif
