#pragma once

#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

namespace errno_error {
namespace detail {

#ifndef _WIN32
// strerror_r has different return types on GNU (char*) vs POSIX XSI (int).
inline std::string strerror_result(int ret, char* buffer, int errnum) {
    if (ret == 0) {
        return std::string(buffer);
    }
    return "errno " + std::to_string(errnum);
}

inline std::string strerror_result(char* ret, char*, int errnum) {
    if (ret != nullptr) {
        return std::string(ret);
    }
    return "errno " + std::to_string(errnum);
}
#endif

inline std::string fallback_message(int errnum) {
    return "errno " + std::to_string(errnum);
}

} // namespace detail

inline std::string message_from_errno(int errnum) {
#ifdef _WIN32
    const char* message = std::strerror(errnum);
    if (message != nullptr) {
        return std::string(message);
    }
    return detail::fallback_message(errnum);
#else
    char buffer[256];
    buffer[0] = '\0';
    return detail::strerror_result(strerror_r(errnum, buffer, sizeof(buffer)), buffer, errnum);
#endif
}

inline std::string operation_failed(const std::string& operation, int errnum) {
    return operation + " failed: " + message_from_errno(errnum);
}

} // namespace errno_error

namespace platform {

std::uint64_t monotonic_ms();
void sleep_ms(unsigned long ms);

std::string hostname();
std::string platform_name();
std::string arch_name();
bool is_windows();

bool is_absolute_path(const std::string& path);
std::string normalize_path_separators(std::string path);

bool shell_supported(const std::string& shell);
std::string resolve_default_shell(const std::string& configured_default_shell);
std::string selected_shell(const std::string& shell_override, const std::string& default_shell);
std::vector<std::string> shell_argv(
    const std::string& shell,
    bool login,
    const std::string& command
);

} // namespace platform
