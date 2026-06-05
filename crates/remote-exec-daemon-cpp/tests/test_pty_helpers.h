#pragma once

#include <cctype>
#include <cstdio>
#include <cstdint>
#include <string>

#ifdef _WIN32
#include <windows.h>
#endif

#include "platform/platform.h"
#include "test_assert.h"
#include "test_filesystem.h"

namespace test_exec_pty {

inline std::string compact_pty_size_output(const std::string& input) {
    std::string output;
    output.reserve(input.size());
    bool previous_space = true;
    for (std::string::const_iterator it = input.begin(); it != input.end(); ++it) {
        const unsigned char ch = static_cast<unsigned char>(*it);
        const bool separator = ch == '\0' || ch == '\r' || ch == '\n' || ch == '\t' || ch == ';' || ch == '=' ||
                               ch == ',';
        if (separator || std::isspace(ch)) {
            if (!previous_space) {
                output.push_back(' ');
                previous_space = true;
            }
            continue;
        }
        output.push_back(static_cast<char>(std::tolower(ch)));
        previous_space = false;
    }
    if (!output.empty() && output[output.size() - 1] == ' ') {
        output.erase(output.size() - 1);
    }
    return output;
}

inline bool pty_size_output_matches(const std::string& output, unsigned short rows, unsigned short cols) {
    const std::string compact = compact_pty_size_output(output);
    const std::string row_text = std::to_string(rows);
    const std::string col_text = std::to_string(cols);
    return compact.find(row_text + " " + col_text) != std::string::npos ||
           (compact.find("rows " + row_text) != std::string::npos &&
            compact.find("columns " + col_text) != std::string::npos) ||
           (compact.find(row_text + " rows") != std::string::npos &&
            compact.find(col_text + " columns") != std::string::npos);
}

inline bool wait_until_file_contains(const test_fs::path& path, const std::string& fragment, unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (test_fs::exists(path) && test_fs::read_file_bytes(path).find(fragment) != std::string::npos) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return test_fs::exists(path) && test_fs::read_file_bytes(path).find(fragment) != std::string::npos;
}

inline std::string terminal_input_line(const std::string& value) {
#ifdef _WIN32
    return value + "\r\n";
#else
    return value + "\n";
#endif
}

#ifdef _WIN32
inline std::string windows_ping_sleep_command(unsigned long seconds) {
    return "ping -n " + std::to_string(seconds + 1UL) + " 127.0.0.1>nul";
}

inline bool is_wine_runtime() {
    const HMODULE ntdll = GetModuleHandleW(L"ntdll.dll");
    return ntdll != nullptr && GetProcAddress(ntdll, "wine_get_version") != nullptr;
}

inline void assert_built_winpty_runtime_available(bool runtime_supports_pty) {
#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
    if (!is_wine_runtime()) {
        TEST_ASSERT(runtime_supports_pty);
    }
#else
    (void)runtime_supports_pty;
#endif
}
#endif

inline bool should_skip_pty_tests(bool runtime_supports_pty) {
#ifdef _WIN32
#ifdef REMOTE_EXEC_CPP_HAS_WINPTY
    if (!runtime_supports_pty && is_wine_runtime()) {
        static bool warned = false;
        if (!warned) {
            std::fprintf(stderr, "warning: skipping PTY tests under Wine; WinPTY TTY support is unavailable\n");
            warned = true;
        }
        return true;
    }
    assert_built_winpty_runtime_available(runtime_supports_pty);
#endif
#endif
    return !runtime_supports_pty;
}

} // namespace test_exec_pty
