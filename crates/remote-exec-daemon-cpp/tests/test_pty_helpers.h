#pragma once

#include <cctype>
#include <cstdio>
#include <cstdint>
#include <cstdlib>
#include <string>
#include <vector>

#ifdef _WIN32
#include <limits.h>
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
inline std::string windows_quote_arg(const std::string& arg) {
    if (arg.empty()) {
        return "\"\"";
    }

    bool needs_quotes = false;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        if (arg[i] == ' ' || arg[i] == '\t' || arg[i] == '"') {
            needs_quotes = true;
            break;
        }
    }
    if (!needs_quotes) {
        return arg;
    }

    std::string quoted = "\"";
    std::size_t backslashes = 0U;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        const char ch = arg[i];
        if (ch == '\\') {
            ++backslashes;
            continue;
        }
        if (ch == '"') {
            quoted.append(backslashes * 2U + 1U, '\\');
            quoted.push_back('"');
            backslashes = 0U;
            continue;
        }
        quoted.append(backslashes, '\\');
        backslashes = 0U;
        quoted.push_back(ch);
    }
    quoted.append(backslashes * 2U, '\\');
    quoted.push_back('"');
    return quoted;
}

inline std::string test_executable_path() {
    std::vector<wchar_t> buffer(MAX_PATH);
    DWORD length = GetModuleFileNameW(nullptr, &buffer[0], static_cast<DWORD>(buffer.size()));
    while (length == buffer.size()) {
        buffer.resize(buffer.size() * 2U);
        length = GetModuleFileNameW(nullptr, &buffer[0], static_cast<DWORD>(buffer.size()));
    }
    TEST_ASSERT(length != 0U);
    return test_fs::utf8_from_wide(std::wstring(buffer.begin(), buffer.begin() + length));
}

inline std::string quoted_test_executable_path() {
    return windows_quote_arg(test_executable_path());
}

inline std::string windows_ping_sleep_command(unsigned long seconds) {
    return "ping -n " + std::to_string(seconds + 1UL) + " 127.0.0.1>nul";
}

inline std::string windows_stdin_echo_helper_command(const std::string& helper_arg, const std::string& label) {
    return quoted_test_executable_path() + " " + windows_quote_arg(helper_arg) + " stdin-echo " + windows_quote_arg(label);
}

inline std::string windows_stdin_echo_sleep_helper_command(const std::string& helper_arg,
                                                           const std::string& label,
                                                           unsigned long sleep_seconds) {
    return quoted_test_executable_path() + " " + windows_quote_arg(helper_arg) + " stdin-echo-sleep " +
           windows_quote_arg(label) + " " + std::to_string(sleep_seconds);
}

inline std::string windows_stdin_file_helper_command(const std::string& helper_arg,
                                                     const std::string& file_name,
                                                     unsigned long sleep_seconds) {
    return quoted_test_executable_path() + " " + windows_quote_arg(helper_arg) + " stdin-file " +
           windows_quote_arg(file_name) + " " + std::to_string(sleep_seconds);
}

inline std::string windows_tty_flag_helper_command(const std::string& helper_arg, const std::string& flag_file_name) {
    return quoted_test_executable_path() + " " + windows_quote_arg(helper_arg) + " tty-flag " +
           windows_quote_arg(flag_file_name);
}

inline std::string windows_resize_helper_command(const std::string& helper_arg) {
    return quoted_test_executable_path() + " " + windows_quote_arg(helper_arg) + " resize";
}

inline std::string read_stdin_line_for_helper() {
    std::string line;
    for (;;) {
        const int ch = std::fgetc(stdin);
        if (ch == EOF || ch == '\n') {
            break;
        }
        if (ch != '\r') {
            line.push_back(static_cast<char>(ch));
        }
    }
    return line;
}

inline unsigned long parse_helper_sleep_seconds(const char* text) {
    char* end = nullptr;
    const unsigned long value = std::strtoul(text, &end, 10);
    TEST_ASSERT(end != text && end != nullptr && *end == '\0');
    return value;
}

inline int run_windows_stdin_helper(int argc, char** argv, int mode_index) {
    TEST_ASSERT(argc > mode_index);
    const std::string mode = argv[mode_index];
    if (mode == "stdin-echo") {
        TEST_ASSERT(argc == mode_index + 2);
        std::printf("ready\n");
        std::fflush(stdout);
        const std::string line = read_stdin_line_for_helper();
        std::printf("%s:%s\n", argv[mode_index + 1], line.c_str());
        std::fflush(stdout);
        return 0;
    }
    if (mode == "stdin-echo-sleep") {
        TEST_ASSERT(argc == mode_index + 3);
        std::printf("ready\n");
        std::fflush(stdout);
        const std::string line = read_stdin_line_for_helper();
        std::printf("%s:%s\n", argv[mode_index + 1], line.c_str());
        std::fflush(stdout);
        const unsigned long sleep_seconds = parse_helper_sleep_seconds(argv[mode_index + 2]);
        platform::sleep_ms(sleep_seconds * 1000UL);
        return 0;
    }
    if (mode == "stdin-file") {
        TEST_ASSERT(argc == mode_index + 3);
        const std::string line = read_stdin_line_for_helper();
        test_fs::write_file_bytes(test_fs::path(argv[mode_index + 1]), line);
        const unsigned long sleep_seconds = parse_helper_sleep_seconds(argv[mode_index + 2]);
        platform::sleep_ms(sleep_seconds * 1000UL);
        return 0;
    }
    if (mode == "tty-flag") {
        TEST_ASSERT(argc == mode_index + 2);
        test_fs::write_file_bytes(test_fs::path(argv[mode_index + 1]), "yes");
        std::printf("tty:yes\n");
        std::fflush(stdout);
        const std::string line = read_stdin_line_for_helper();
        std::printf("%s\ninput:%s\n", line.c_str(), line.c_str());
        std::fflush(stdout);
        return 0;
    }
    if (mode == "resize") {
        TEST_ASSERT(argc == mode_index + 1);
        std::printf("ready\nrows=24 cols=120\n");
        std::fflush(stdout);
        (void)read_stdin_line_for_helper();
        std::printf("rows=33 cols=101\n");
        std::fflush(stdout);
        platform::sleep_ms(30000UL);
        return 0;
    }

    TEST_ASSERT(false && "unknown Windows stdin helper mode");
    return 2;
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
