#pragma once

#include <string>

#include <windows.h>

std::string read_available_console_output(HANDLE pipe, std::string* carry);
std::string read_console_output(HANDLE pipe, bool block, bool* eof, std::string* carry);
std::string flush_console_output_carry(std::string* carry);

class TerminalOutputFilter {
public:
    TerminalOutputFilter();

    std::string filter_chunk(const std::string& chunk);
    std::string drain_pending();

private:
    enum class State {
        Ground,
        Escape,
        EscapeIntermediate,
        Csi,
        OscString,
        IgnoreString,
    };

    void process_byte(unsigned char byte, std::string* output);
    void emit_control(unsigned char byte, std::string* output) const;

    State state_;
};

#ifdef REMOTE_EXEC_CPP_TESTING
std::string utf8_from_windows_wide_for_test(const std::wstring& wide);
std::string utf8_from_windows_code_page_for_test(unsigned int code_page, const std::string& raw);
std::string decode_console_output_for_test(unsigned int primary_code_page,
                                           unsigned int fallback_code_page,
                                           std::string* carry,
                                           const std::string& raw_chunk,
                                           bool flush);
std::string decode_utf8_stream_for_test(std::string* carry, const std::string& raw_chunk, bool flush);
std::string filter_terminal_output_for_test(TerminalOutputFilter* filter, const std::string& chunk);
std::string drain_terminal_output_for_test(TerminalOutputFilter* filter);
#endif
