#include <algorithm>
#include <cctype>
#include <cstdint>
#include <string>

#ifdef _WIN32
#include <windows.h>
#include <winsock2.h>
#else
#include <cctype>
#include <errno.h>
#include <sys/time.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>
#endif

#include "platform.h"

namespace {

#ifndef _WIN32
std::string lowercase_ascii(std::string value) {
    for (std::size_t i = 0; i < value.size(); ++i) {
        value[i] = static_cast<char>(std::tolower(static_cast<unsigned char>(value[i])));
    }
    return value;
}
#endif

} // namespace

namespace platform {

std::uint64_t monotonic_ms() {
#ifdef _WIN32
    return static_cast<std::uint64_t>(GetTickCount());
#else
#ifdef CLOCK_MONOTONIC
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) == 0) {
        return static_cast<std::uint64_t>(ts.tv_sec) * 1000ULL + static_cast<std::uint64_t>(ts.tv_nsec / 1000000L);
    }
#endif
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return static_cast<std::uint64_t>(tv.tv_sec) * 1000ULL + static_cast<std::uint64_t>(tv.tv_usec / 1000L);
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
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 && path[1] == ':' &&
            (path[2] == '\\' || path[2] == '/')) ||
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

} // namespace platform
