#pragma once

#include <cstdint>
#include <string>
#include <vector>

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
std::string selected_shell(
    const std::string& shell_override,
    const std::string& default_shell
);
std::vector<std::string> shell_argv(
    const std::string& shell,
    bool login,
    const std::string& command
);

}  // namespace platform
