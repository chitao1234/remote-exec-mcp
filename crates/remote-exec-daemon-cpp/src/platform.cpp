#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <errno.h>
#include <pwd.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#endif

#include "platform.h"

namespace {

std::string lowercase_ascii(std::string value) {
    for (std::size_t i = 0; i < value.size(); ++i) {
        value[i] = static_cast<char>(std::tolower(static_cast<unsigned char>(value[i])));
    }
    return value;
}

#ifdef _WIN32
std::string shell_basename_lower(const std::string& shell) {
    const std::size_t slash = shell.find_last_of("/\\");
    const std::string base = slash == std::string::npos ? shell : shell.substr(slash + 1);
    return lowercase_ascii(base);
}

bool is_windows_cmd_family(const std::string& lower) {
    return lower == "cmd.exe" || lower == "cmd";
}

bool is_windows_powershell_family(const std::string& lower) {
    return lower == "powershell.exe" || lower == "powershell" ||
           lower == "pwsh.exe" || lower == "pwsh";
}
#endif

#ifndef _WIN32
bool is_path_like(const std::string& command) {
    return command.find('/') != std::string::npos || command.find('\\') != std::string::npos ||
           platform::is_absolute_path(command);
}

bool is_disallowed_unix_shell(const std::string& shell) {
    const std::size_t slash = shell.find_last_of('/');
    const std::string base = slash == std::string::npos ? shell : shell.substr(slash + 1);
    return base == "false" || base == "nologin";
}

bool is_executable_file(const std::string& path) {
    struct stat st;
    return stat(path.c_str(), &st) == 0 && S_ISREG(st.st_mode) && access(path.c_str(), X_OK) == 0;
}

bool probe_unix_shell(const std::string& shell) {
    const pid_t pid = fork();
    if (pid < 0) {
        return false;
    }
    if (pid == 0) {
        execl(shell.c_str(), shell.c_str(), "-c", "exit 0", static_cast<char*>(NULL));
        _exit(127);
    }

    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) {
            return false;
        }
    }
    return WIFEXITED(status) && WEXITSTATUS(status) == 0;
}

std::string find_command_on_path(const std::string& command) {
    const char* path_env = std::getenv("PATH");
    if (path_env == NULL || command.empty()) {
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
        const std::string candidate = platform::join_path(dir, command);
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

std::string passwd_shell() {
    struct passwd* entry = getpwuid(geteuid());
    if (entry == NULL || entry->pw_shell == NULL || entry->pw_shell[0] == '\0') {
        return "";
    }
    return entry->pw_shell;
}
#endif

}  // namespace

namespace platform {

std::uint64_t monotonic_ms() {
#ifdef _WIN32
    return static_cast<std::uint64_t>(GetTickCount());
#else
#ifdef CLOCK_MONOTONIC
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) == 0) {
        return static_cast<std::uint64_t>(ts.tv_sec) * 1000ULL +
               static_cast<std::uint64_t>(ts.tv_nsec / 1000000L);
    }
#endif
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return static_cast<std::uint64_t>(tv.tv_sec) * 1000ULL +
           static_cast<std::uint64_t>(tv.tv_usec / 1000L);
#endif
}

void sleep_ms(unsigned long ms) {
#ifdef _WIN32
    Sleep(ms);
#else
    struct timespec requested;
    requested.tv_sec = static_cast<time_t>(ms / 1000UL);
    requested.tv_nsec = static_cast<long>((ms % 1000UL) * 1000000UL);
    while (nanosleep(&requested, &requested) != 0 && errno == EINTR) {
    }
#endif
}

std::string hostname() {
#ifdef _WIN32
    char buffer[MAX_COMPUTERNAME_LENGTH + 1];
    DWORD size = MAX_COMPUTERNAME_LENGTH + 1;
    if (GetComputerNameA(buffer, &size) == 0) {
        return "windows";
    }
    return std::string(buffer, size);
#else
    char buffer[256];
    if (gethostname(buffer, sizeof(buffer)) != 0) {
        return "posix";
    }
    buffer[sizeof(buffer) - 1] = '\0';
    return buffer;
#endif
}

std::string platform_name() {
#ifdef _WIN32
    return "windows";
#else
    struct utsname uts;
    if (uname(&uts) == 0) {
        std::string sysname = lowercase_ascii(uts.sysname);
        return sysname.empty() ? "unix" : sysname;
    }
    return "unix";
#endif
}

std::string arch_name() {
#ifdef _WIN32
#if defined(_M_X64) || defined(__x86_64__)
    return "x86_64";
#elif defined(_M_IX86) || defined(__i386__)
    return "x86";
#elif defined(_M_ARM64) || defined(__aarch64__)
    return "aarch64";
#else
    return "unknown";
#endif
#else
    struct utsname uts;
    if (uname(&uts) == 0 && uts.machine[0] != '\0') {
        return uts.machine;
    }
    return "unknown";
#endif
}

bool is_windows() {
#ifdef _WIN32
    return true;
#else
    return false;
#endif
}

bool is_absolute_path(const std::string& path) {
#ifdef _WIN32
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 &&
            path[1] == ':' && (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
#else
    return !path.empty() && path[0] == '/';
#endif
}

std::string normalize_path_separators(std::string path) {
#ifdef _WIN32
    std::replace(path.begin(), path.end(), '/', '\\');
#endif
    return path;
}

std::string join_path(const std::string& base, const std::string& relative) {
    if (base.empty()) {
        return normalize_path_separators(relative);
    }

    std::string joined = normalize_path_separators(base);
#ifdef _WIN32
    const char separator = '\\';
#else
    const char separator = '/';
#endif
    if (!joined.empty() && joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(separator);
    }
    joined += normalize_path_separators(relative);
    return joined;
}

bool shell_supported(const std::string& shell) {
#ifdef _WIN32
    return is_windows_cmd_family(shell_basename_lower(shell));
#else
    (void)shell;
    return true;
#endif
}

std::string resolve_default_shell(const std::string& configured_default_shell) {
#ifdef _WIN32
    if (!configured_default_shell.empty()) {
        if (!shell_supported(configured_default_shell)) {
            throw std::runtime_error("only cmd.exe is supported on this Windows C++ daemon build");
        }
        return configured_default_shell;
    }
    const char* comspec = std::getenv("COMSPEC");
    if (comspec != NULL && comspec[0] != '\0' && shell_supported(comspec)) {
        return comspec;
    }
    return "cmd.exe";
#else
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
    const char* candidates[] = {
        env_shell,
        NULL,
        "bash",
        "/bin/sh",
    };
    const std::string passwd = passwd_shell();
    candidates[1] = passwd.empty() ? NULL : passwd.c_str();

    for (std::size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); ++i) {
        if (candidates[i] == NULL || candidates[i][0] == '\0') {
            continue;
        }
        const std::string resolved = validate_unix_shell_candidate(candidates[i]);
        if (!resolved.empty()) {
            return resolved;
        }
    }

    throw std::runtime_error(
        "no usable default shell found; tried SHELL, passwd shell, bash, and /bin/sh"
    );
#endif
}

std::string selected_shell(
    const std::string& shell_override,
    const std::string& default_shell
) {
    const std::string shell = shell_override.empty() ? default_shell : shell_override;
    if (!shell_supported(shell)) {
        throw std::runtime_error("unsupported shell `" + shell + "`");
    }
    return shell;
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
    if (is_windows_cmd_family(lower)) {
        if (!login) {
            argv.push_back("/D");
        }
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

}  // namespace platform
