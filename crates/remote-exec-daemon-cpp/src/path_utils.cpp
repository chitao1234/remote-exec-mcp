#include "path_utils.h"

#include <algorithm>
#include <cstring>
#include <cerrno>
#include <stdexcept>

#ifdef _WIN32
#include <direct.h>
#include <io.h>
#include <wchar.h>
#include <windows.h>
#else
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif

namespace path_utils {

#ifdef _WIN32
namespace {

const unsigned int RENAME_RETRY_ATTEMPTS = 8U;
const unsigned long RENAME_RETRY_MAX_DELAY_MS = 16UL;

std::wstring wide_mode_from_ascii(const char* mode) {
    std::wstring wide;
    while (mode != nullptr && *mode != '\0') {
        wide.push_back(static_cast<unsigned char>(*mode));
        ++mode;
    }
    return wide;
}

int last_error_to_errno(DWORD error) {
    switch (error) {
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:
    case ERROR_INVALID_NAME:
    case ERROR_BAD_PATHNAME:
    case ERROR_DIRECTORY:
        return ENOENT;
    case ERROR_ACCESS_DENIED:
    case ERROR_SHARING_VIOLATION:
    case ERROR_LOCK_VIOLATION:
        return EACCES;
    default:
        return EIO;
    }
}

bool should_retry_rename_error(DWORD error) {
    return error == ERROR_ACCESS_DENIED || error == ERROR_SHARING_VIOLATION || error == ERROR_LOCK_VIOLATION;
}

} // namespace

std::wstring wide_from_utf8(const std::string& value) {
    if (value.empty()) {
        return std::wstring();
    }

    const int wide_length =
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), nullptr, 0);
    if (wide_length <= 0) {
        throw std::runtime_error("unable to decode UTF-8 path");
    }

    std::wstring wide(static_cast<std::size_t>(wide_length), L'\0');
    if (MultiByteToWideChar(
            CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), &wide[0], wide_length) <=
        0) {
        throw std::runtime_error("unable to decode UTF-8 path");
    }
    return wide;
}

std::string utf8_from_wide(const std::wstring& value) {
    if (value.empty()) {
        return std::string();
    }

    const int utf8_length =
        WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    if (utf8_length <= 0) {
        throw std::runtime_error("unable to encode UTF-8 path");
    }

    std::string utf8(static_cast<std::size_t>(utf8_length), '\0');
    if (WideCharToMultiByte(
            CP_UTF8, 0, value.data(), static_cast<int>(value.size()), &utf8[0], utf8_length, nullptr, nullptr) <= 0) {
        throw std::runtime_error("unable to encode UTF-8 path");
    }
    return utf8;
}
#endif

char native_separator() {
#ifdef _WIN32
    return '\\';
#else
    return '/';
#endif
}

std::string parent_directory(const std::string& path) {
    const std::size_t slash = path.find_last_of("/\\");
    if (slash == std::string::npos) {
        return "";
    }
    return path.substr(0, slash);
}

std::string join_path(const std::string& base, const std::string& child) {
    std::string normalized_child = child;
#ifdef _WIN32
    std::replace(normalized_child.begin(), normalized_child.end(), '/', '\\');
#endif
    if (base.empty()) {
        return normalized_child;
    }
    std::string joined = base;
#ifdef _WIN32
    std::replace(joined.begin(), joined.end(), '/', '\\');
#endif
    if (joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(native_separator());
    }
    joined += normalized_child;
    return joined;
}

void make_directory_if_missing(const std::string& path) {
    if (path.empty()) {
        return;
    }
#ifdef _WIN32
    if (_wmkdir(wide_from_utf8(path).c_str()) != 0 && errno != EEXIST) {
#else
    if (mkdir(path.c_str(), 0777) != 0 && errno != EEXIST) {
#endif
        throw std::runtime_error("unable to create directory " + path);
    }
}

void create_parent_directories(const std::string& path) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }

    std::string current;
    for (std::size_t i = 0; i < parent.size(); ++i) {
        const char ch = parent[i];
        current.push_back(ch);
        if (ch != '/' && ch != '\\') {
            continue;
        }
        if (current.size() == 1) {
            continue;
        }
        if (current.size() == 3 && current[1] == ':') {
            continue;
        }
        current.erase(current.size() - 1);
        make_directory_if_missing(current);
        current.push_back(ch);
    }
    make_directory_if_missing(parent);
}

FILE* open_file(const std::string& path, const char* mode) {
#ifdef _WIN32
    return _wfopen(wide_from_utf8(path).c_str(), wide_mode_from_ascii(mode).c_str());
#else
    return std::fopen(path.c_str(), mode);
#endif
}

bool stat_path(const std::string& path, struct stat* st) {
#ifdef _WIN32
    WIN32_FILE_ATTRIBUTE_DATA data;
    if (!GetFileAttributesExW(wide_from_utf8(path).c_str(), GetFileExInfoStandard, &data)) {
        errno = last_error_to_errno(GetLastError());
        return false;
    }

    std::memset(st, 0, sizeof(*st));
    st->st_mode = (data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0 ? S_IFDIR : S_IFREG;
    if ((data.dwFileAttributes & FILE_ATTRIBUTE_READONLY) != 0) {
        st->st_mode |= 0444;
    } else {
        st->st_mode |= 0666;
    }
    st->st_size = static_cast<_off_t>(
        (static_cast<unsigned long long>(data.nFileSizeHigh) << 32) | data.nFileSizeLow);
    return true;
#else
    return stat(path.c_str(), st) == 0;
#endif
}

bool lstat_path(const std::string& path, struct stat* st) {
#ifdef _WIN32
    return stat_path(path, st);
#else
    return lstat(path.c_str(), st) == 0;
#endif
}

bool remove_path(const std::string& path) {
#ifdef _WIN32
    return _wremove(wide_from_utf8(path).c_str()) == 0;
#else
    return std::remove(path.c_str()) == 0;
#endif
}

bool remove_directory(const std::string& path) {
#ifdef _WIN32
    return _wrmdir(wide_from_utf8(path).c_str()) == 0;
#else
    return rmdir(path.c_str()) == 0;
#endif
}

bool rename_path(const std::string& source, const std::string& destination) {
#ifdef _WIN32
    const std::wstring wide_source = wide_from_utf8(source);
    const std::wstring wide_destination = wide_from_utf8(destination);
    DWORD last_error = ERROR_SUCCESS;
    unsigned long retry_delay_ms = 1UL;

    for (unsigned int attempt = 0U; attempt < RENAME_RETRY_ATTEMPTS; ++attempt) {
        if (MoveFileExW(wide_source.c_str(), wide_destination.c_str(), MOVEFILE_REPLACE_EXISTING) != 0) {
            return true;
        }
        last_error = GetLastError();
        if (!should_retry_rename_error(last_error) || attempt + 1U == RENAME_RETRY_ATTEMPTS) {
            errno = last_error_to_errno(last_error);
            return false;
        }
        Sleep(retry_delay_ms);
        if (retry_delay_ms < RENAME_RETRY_MAX_DELAY_MS) {
            retry_delay_ms *= 2UL;
        }
    }

    errno = last_error_to_errno(last_error);
    return false;
#else
    return std::rename(source.c_str(), destination.c_str()) == 0;
#endif
}

} // namespace path_utils
