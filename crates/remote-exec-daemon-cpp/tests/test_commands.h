#pragma once

#include <fstream>
#include <sstream>
#include <string>

#include "test_filesystem.h"

namespace test_commands {

inline unsigned long seconds_for_delay(unsigned long milliseconds) {
    return milliseconds == 0UL ? 1UL : (milliseconds + 999UL) / 1000UL;
}

// A single shell command that waits without producing output. Windows 9x
// COMMAND.COM does not support `&` or `&&`, so callers must not chain
// commands when this runs on Windows.
inline std::string delay_command(unsigned long milliseconds) {
#ifdef _WIN32
    const unsigned long count = seconds_for_delay(milliseconds) + 1UL;
    std::ostringstream command;
    command << "ping -n " << count << " 127.0.0.1 >nul";
    return command.str();
#else
    std::ostringstream command;
    command << "sleep " << seconds_for_delay(milliseconds);
    return command.str();
#endif
}

inline std::string quote_shell_path(const test_fs::path& path) {
#ifdef _WIN32
    return "\"" + path.string() + "\"";
#else
    std::string quoted("'");
    const std::string value = path.string();
    for (std::string::size_type i = 0; i < value.size(); ++i) {
        if (value[i] == '\'') {
            quoted += "'\\''";
        } else {
            quoted.push_back(value[i]);
        }
    }
    quoted += "'";
    return quoted;
#endif
}

// Returns a command that waits and then writes `text` to stdout. On Windows
// the delay and echo go into a small batch file because COMMAND.COM cannot
// chain commands. On POSIX one shell command is enough.
inline std::string echo_after_delay(
    const test_fs::path& root,
    const std::string& text,
    unsigned long milliseconds
) {
#ifdef _WIN32
    const test_fs::path script = root / "echo_after_delay.bat";
    std::ofstream output(script.string().c_str(), std::ios::binary | std::ios::trunc);
    output << "@echo off\r\n";
    output << delay_command(milliseconds) << "\r\n";
    output << "echo " << text << "\r\n";
    output.close();
    return quote_shell_path(script);
#else
    (void)root;
    std::ostringstream command;
    command << delay_command(milliseconds) << "; printf '%s\\n' '" << text << "'";
    return command.str();
#endif
}

} // namespace test_commands
