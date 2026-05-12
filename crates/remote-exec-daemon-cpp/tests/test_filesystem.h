#ifndef REMOTE_EXEC_TEST_FILESYSTEM_H
#define REMOTE_EXEC_TEST_FILESYSTEM_H

#ifndef _WIN32

#include <filesystem>

namespace test_fs = std::filesystem;

#else

#include <cstdlib>
#include <stdexcept>
#include <string>
#include <vector>

#include <windows.h>

namespace test_fs {

class path {
public:
    path() {}
    path(const char* value) : value_(value == NULL ? "" : value) {}
    path(const std::string& value) : value_(value) {}

    std::string string() const { return value_; }

    const char* c_str() const { return value_.c_str(); }

    path parent_path() const {
        const std::string::size_type slash = value_.find_last_of("/\\");
        if (slash == std::string::npos) {
            return path();
        }
        return path(value_.substr(0, slash));
    }

private:
    std::string value_;
};

inline path operator/(const path& base, const std::string& child) {
    std::string joined = base.string();
    if (!joined.empty() && joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back('\\');
    }
    joined += child;
    return path(joined);
}

inline path operator/(const path& base, const char* child) {
    return base / std::string(child == NULL ? "" : child);
}

inline path temp_directory_path() {
    const char* temp = std::getenv("TEMP");
    if (temp == NULL || temp[0] == '\0') {
        temp = std::getenv("TMP");
    }
    if (temp == NULL || temp[0] == '\0') {
        return path(".");
    }
    return path(temp);
}

inline bool exists(const path& target) {
    return GetFileAttributesA(target.c_str()) != INVALID_FILE_ATTRIBUTES;
}

inline bool is_directory(const path& target) {
    const DWORD attributes = GetFileAttributesA(target.c_str());
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
}

inline void throw_last_error(const std::string& operation, const path& target) {
    throw std::runtime_error(operation + " failed for `" + target.string() +
                             "`: " + std::to_string(static_cast<unsigned long>(GetLastError())));
}

inline void create_directory_if_missing(const path& target) {
    if (target.string().empty() || exists(target)) {
        return;
    }
    if (!CreateDirectoryA(target.c_str(), NULL) && GetLastError() != ERROR_ALREADY_EXISTS) {
        throw_last_error("CreateDirectoryA", target);
    }
}

inline void create_directories(const path& target) {
    const std::string text = target.string();
    if (text.empty() || exists(target)) {
        return;
    }

    std::size_t start = 0;
    if (text.size() >= 2 && text[1] == ':') {
        start = 2;
    }
    while (start < text.size() && (text[start] == '/' || text[start] == '\\')) {
        ++start;
    }

    for (std::size_t index = start; index <= text.size(); ++index) {
        if (index != text.size() && text[index] != '/' && text[index] != '\\') {
            continue;
        }
        const std::string prefix = text.substr(0, index);
        if (!prefix.empty() && !(prefix.size() == 2 && prefix[1] == ':')) {
            create_directory_if_missing(path(prefix));
        }
    }
}

inline void remove_all(const path& target) {
    const DWORD attributes = GetFileAttributesA(target.c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES) {
        return;
    }

    if ((attributes & FILE_ATTRIBUTE_DIRECTORY) == 0) {
        if (!DeleteFileA(target.c_str())) {
            throw_last_error("DeleteFileA", target);
        }
        return;
    }

    const path search = target / "*";
    WIN32_FIND_DATAA data;
    HANDLE handle = FindFirstFileA(search.c_str(), &data);
    if (handle != INVALID_HANDLE_VALUE) {
        do {
            const std::string name(data.cFileName);
            if (name == "." || name == "..") {
                continue;
            }
            remove_all(target / name);
        } while (FindNextFileA(handle, &data));
        FindClose(handle);
    }

    if (!RemoveDirectoryA(target.c_str())) {
        throw_last_error("RemoveDirectoryA", target);
    }
}

} // namespace test_fs

#endif

#endif
