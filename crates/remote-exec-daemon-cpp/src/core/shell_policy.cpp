#include <cstdlib>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#include <cctype>
#include <windows.h>
#else
#include <errno.h>
#ifndef __ANDROID__
#include <pwd.h>
#endif
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

#include "core/shell_policy_internal.h"
#include "platform/path_utils.h"
#include "platform/platform.h"
#include "policy/path_compare.h"
#include "policy/path_policy.h"
#ifndef _WIN32
#include "platform/posix_fd.h"
#include "platform/posix_process.h"
#endif
#include "core/text_utils.h"

#ifndef _WIN32
extern char** environ;
#endif

namespace {

#ifdef _WIN32
using platform_detail::is_windows_bash_family;
using platform_detail::is_windows_cmd_family;
using platform_detail::is_windows_command_family;
using platform_detail::shell_basename_lower;

bool is_windows_powershell_family(const std::string& lower) {
    return lower == "powershell.exe" || lower == "powershell" || lower == "pwsh.exe"
           || lower == "pwsh";
}

bool is_windows_nt_version(DWORD version) {
    return (version & 0x80000000UL) == 0;
}

std::string windows_default_shell_for_version(DWORD version) {
    return is_windows_nt_version(version) ? "cmd.exe" : "command.com";
}

std::string windows_default_shell_fallback() {
    return windows_default_shell_for_version(GetVersion());
}

std::string resolve_windows_shell_path(
    const std::string& shell,
    const std::string& windows_posix_root
) {
    std::string resolved;
    if (resolve_absolute_input_path_for_policy(
            windows_path_policy(),
            shell,
            windows_posix_root,
            &resolved
        )) {
        return resolved;
    }
    return shell;
}
#endif

#ifndef _WIN32
bool is_path_like(const std::string& command) {
    return command.find('/') != std::string::npos || command.find('\\') != std::string::npos
           || platform::is_absolute_path(command);
}

bool is_disallowed_unix_shell(const std::string& shell) {
    const std::size_t slash = shell.find_last_of('/');
    const std::string base = slash == std::string::npos ? shell : shell.substr(slash + 1);
    return base == "false" || base == "nologin";
}

bool is_executable_file(const std::string& path) {
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata(path, &metadata) && metadata.is_regular_file
           && posix_fd::access_path(path.c_str(), X_OK) == 0;
}

bool probe_unix_shell(const std::string& shell) {
    const pid_t pid = posix_process::fork_process();
    if (pid < 0) {
        return false;
    }
    if (pid == 0) {
        char* const argv[] = {
            const_cast<char*>(shell.c_str()),
            const_cast<char*>("-c"),
            const_cast<char*>("exit 0"),
            nullptr,
        };
        posix_process::execve_process(shell.c_str(), argv, environ);
        _exit(127);
    }

    int status = 0;
    if (posix_process::wait_pid(pid, &status, 0) < 0) {
        return false;
    }
    return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

std::string find_command_on_path(const std::string& command) {
    const char* path_env = std::getenv("PATH");
    if (path_env == nullptr || command.empty()) {
        return "";
    }

    std::string current;
    const std::string path(path_env);
    for (std::size_t i = 0; i <= path.size(); ++i) {
        if (i != path.size() && path[i] != ':') {
            current.push_back(path[i]);
            continue;
        }

        const std::string dir = current.empty() ? "." : current;
        const std::string candidate = path_utils::join_path(dir, command);
        if (is_executable_file(candidate)) {
            return candidate;
        }
        current.clear();
    }

    return "";
}

std::string validate_unix_shell_candidate(const std::string& shell) {
    if (shell.empty() || is_disallowed_unix_shell(shell)) {
        return "";
    }
    if (is_path_like(shell)) {
        return is_executable_file(shell) && probe_unix_shell(shell) ? shell : "";
    }
    const std::string resolved = find_command_on_path(shell);
    return !resolved.empty() && probe_unix_shell(resolved) ? resolved : "";
}

#ifndef __ANDROID__
std::string passwd_shell() {
    struct passwd* entry = getpwuid(geteuid());
    if (entry == nullptr || entry->pw_shell == nullptr || entry->pw_shell[0] == '\0') {
        return "";
    }
    return entry->pw_shell;
}
#endif
#endif

} // namespace

namespace platform {

bool shell_supported(const std::string& shell) {
#ifdef _WIN32
    const std::string lower = shell_basename_lower(shell);
    return is_windows_cmd_family(lower) || is_windows_command_family(lower)
           || is_windows_powershell_family(lower) || is_windows_bash_family(lower);
#else
    (void)shell;
    return true;
#endif
}

std::string resolve_default_shell(
    const std::string& configured_default_shell,
    const std::string& windows_posix_root
) {
#ifdef _WIN32
    if (!configured_default_shell.empty()) {
        if (!shell_supported(configured_default_shell)) {
            throw std::runtime_error(
                "supported Windows shells are cmd.exe/cmd, command.com/command, and the "
                "PowerShell family (powershell.exe, powershell, pwsh.exe, pwsh), or "
                "Git Bash (bash.exe, bash, sh.exe, sh, git-bash.exe, git-bash)"
            );
        }
        return resolve_windows_shell_path(configured_default_shell, windows_posix_root);
    }
    const char* comspec = std::getenv("COMSPEC");
    if (comspec != nullptr && comspec[0] != '\0' && shell_supported(comspec)) {
        return comspec;
    }
    return windows_default_shell_fallback();
#else
    (void)windows_posix_root;
    if (!configured_default_shell.empty()) {
        const std::string resolved = validate_unix_shell_candidate(configured_default_shell);
        if (resolved.empty()) {
            throw std::runtime_error(
                "configured default shell `" + configured_default_shell + "` is not usable"
            );
        }
        return resolved;
    }

    const char* env_shell = std::getenv("SHELL");
#ifdef __ANDROID__
    const char* candidates[] = {
        env_shell,
        "bash",
        "sh",
        "/system/bin/sh",
    };
#else
    const char* candidates[] = {
        env_shell,
        nullptr,
        "bash",
        "sh",
        "/bin/sh",
    };
    const std::string passwd = passwd_shell();
    candidates[1] = passwd.empty() ? nullptr : passwd.c_str();
#endif

    for (std::size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); ++i) {
        if (candidates[i] == nullptr || candidates[i][0] == '\0') {
            continue;
        }
        const std::string resolved = validate_unix_shell_candidate(candidates[i]);
        if (!resolved.empty()) {
            return resolved;
        }
    }

#ifdef __ANDROID__
    throw std::runtime_error(
        "no usable default shell found; tried SHELL, bash, sh, and /system/bin/sh"
    );
#else
    throw std::runtime_error(
        "no usable default shell found; tried SHELL, passwd shell, bash, sh, and /bin/sh"
    );
#endif
#endif
}

#if defined(_WIN32) && defined(REMOTE_EXEC_CPP_TESTING)
std::string windows_default_shell_for_version_for_test(unsigned long version) {
    return windows_default_shell_for_version(static_cast<DWORD>(version));
}
#endif

std::string selected_shell(
    const std::string& shell_override,
    const std::string& default_shell,
    const std::string& windows_posix_root
) {
    const std::string shell = shell_override.empty() ? default_shell : shell_override;
    if (!shell_supported(shell)) {
        throw std::runtime_error("unsupported shell `" + shell + "`");
    }
#ifdef _WIN32
    return resolve_windows_shell_path(shell, windows_posix_root);
#else
    (void)windows_posix_root;
    return shell;
#endif
}

bool should_set_chere_invoking(const std::string& shell, const std::string& windows_posix_root) {
#ifdef __CYGWIN__
    (void)shell;
    (void)windows_posix_root;
    return true;
#elif defined(_WIN32)
    if (platform_detail::is_windows_bash_family(platform_detail::shell_basename_lower(shell))) {
        return true;
    }
    return !windows_posix_root.empty() && host_path_is_within(shell, windows_posix_root);
#else
    (void)shell;
    (void)windows_posix_root;
    return false;
#endif
}

std::vector<std::string> shell_argv(
    const std::string& shell,
    bool login,
    const std::string& command
) {
    std::vector<std::string> argv;
    argv.push_back(shell);

#ifdef _WIN32
    const std::string lower = shell_basename_lower(shell);
    if (is_windows_powershell_family(lower)) {
        if (!login) {
            argv.push_back("-NoProfile");
        }
        argv.push_back("-Command");
        argv.push_back(command);
        return argv;
    }
    if (is_windows_bash_family(lower)) {
        if (login) {
            argv.push_back("-l");
        }
        argv.push_back("-c");
        argv.push_back(command);
        return argv;
    }
    if (is_windows_cmd_family(lower)) {
        if (!login) {
            argv.push_back("/D");
        }
        argv.push_back("/C");
        argv.push_back(command);
        return argv;
    }
    if (is_windows_command_family(lower)) {
        argv.push_back("/C");
        argv.push_back(command);
        return argv;
    }
    argv.push_back("/C");
    argv.push_back(command);
#else
    if (login) {
        argv.push_back("-l");
    }
    argv.push_back("-c");
    argv.push_back(command);
#endif

    return argv;
}

} // namespace platform
