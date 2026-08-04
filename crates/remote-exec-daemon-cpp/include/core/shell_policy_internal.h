#pragma once

#ifdef _WIN32

#include <string>

#include "core/text_utils.h"

// Shared Windows shell-family helpers used by shell policy resolution and the
// Windows process session launch path.
namespace platform_detail {

inline std::string shell_basename_lower(const std::string& shell) {
    const std::size_t slash = shell.find_last_of("/\\");
    const std::string base = slash == std::string::npos ? shell : shell.substr(slash + 1);
    return lowercase_ascii(base);
}

inline bool is_windows_cmd_family(const std::string& lower) {
    return lower == "cmd.exe" || lower == "cmd";
}

inline bool is_windows_command_family(const std::string& lower) {
    return lower == "command.com" || lower == "command";
}

} // namespace platform_detail

#endif
