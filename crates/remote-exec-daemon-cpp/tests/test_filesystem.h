#ifndef REMOTE_EXEC_TEST_FILESYSTEM_H
#define REMOTE_EXEC_TEST_FILESYSTEM_H

#include <cstdlib>
#include <cstdio>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#include <direct.h>
#include <wchar.h>
#include <windows.h>
#else
#include <cerrno>
#include <cstring>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif
#include "scoped_file.h"

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
        if (slash == 0U) {
            return path(value_.substr(0, 1U));
        }
        return path(value_.substr(0, slash));
    }

    path filename() const {
        const std::string::size_type slash = value_.find_last_of("/\\");
        if (slash == std::string::npos) {
            return path(value_);
        }
        return path(value_.substr(slash + 1U));
    }

private:
    std::string value_;
};

inline bool operator==(const path& left, const path& right) {
    return left.string() == right.string();
}

inline bool operator!=(const path& left, const path& right) {
    return !(left == right);
}

inline path operator/(const path& base, const std::string& child) {
    std::string joined = base.string();
    if (!joined.empty() && joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
#ifdef _WIN32
        joined.push_back('\\');
#else
        joined.push_back('/');
#endif
    }
    joined += child;
    return path(joined);
}

inline path operator/(const path& base, const char* child) {
    return base / std::string(child == NULL ? "" : child);
}

inline path operator/(const path& base, const path& child) {
    return base / child.string();
}

#ifdef _WIN32
inline std::wstring wide_from_utf8(const std::string& value) {
    if (value.empty()) {
        return std::wstring();
    }

    const int wide_length =
        MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), NULL, 0);
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

inline std::string utf8_from_wide(const std::wstring& value) {
    if (value.empty()) {
        return std::string();
    }

    const int utf8_length =
        WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()), NULL, 0, NULL, NULL);
    if (utf8_length <= 0) {
        throw std::runtime_error("unable to encode UTF-8 path");
    }

    std::string utf8(static_cast<std::size_t>(utf8_length), '\0');
    if (WideCharToMultiByte(
            CP_UTF8, 0, value.data(), static_cast<int>(value.size()), &utf8[0], utf8_length, NULL, NULL) <= 0) {
        throw std::runtime_error("unable to encode UTF-8 path");
    }
    return utf8;
}

inline std::wstring wide_mode_from_ascii(const char* mode) {
    std::wstring wide;
    while (mode != NULL && *mode != '\0') {
        wide.push_back(static_cast<unsigned char>(*mode));
        ++mode;
    }
    return wide;
}
#endif

inline FILE* open_file(const path& target, const char* mode) {
#ifdef _WIN32
    return _wfopen(wide_from_utf8(target.string()).c_str(), wide_mode_from_ascii(mode).c_str());
#else
    return std::fopen(target.c_str(), mode);
#endif
}

inline std::string read_file_bytes(const path& target) {
    ScopedFile file(open_file(target, "rb"));
    if (!file.valid()) {
        throw std::runtime_error("unable to open file " + target.string());
    }

    std::string contents;
    char buffer[4096];
    while (true) {
        const std::size_t received = std::fread(buffer, 1, sizeof(buffer), file.get());
        if (received > 0U) {
            contents.append(buffer, received);
        }
        if (received < sizeof(buffer)) {
            if (std::ferror(file.get()) != 0) {
                throw std::runtime_error("unable to read file " + target.string());
            }
            break;
        }
    }
    return contents;
}

inline void write_file_bytes(const path& target, const std::string& contents) {
    ScopedFile file(open_file(target, "wb"));
    if (!file.valid()) {
        throw std::runtime_error("unable to open file " + target.string());
    }
    if (!contents.empty() && std::fwrite(contents.data(), 1, contents.size(), file.get()) != contents.size()) {
        throw std::runtime_error("unable to write file " + target.string());
    }
    if (file.close() != 0) {
        throw std::runtime_error("unable to write file " + target.string());
    }
}

enum class perms : unsigned {
    none = 0U,
    owner_exec = 0100U,
    group_exec = 0010U,
    others_exec = 0001U,
    owner_all = 0700U,
};

inline perms operator|(perms left, perms right) {
    return static_cast<perms>(static_cast<unsigned>(left) | static_cast<unsigned>(right));
}

enum class perm_options {
    replace,
    add,
};

class file_status {
public:
    explicit file_status(perms mode) : mode_(mode) {}

    perms permissions() const { return mode_; }

private:
    perms mode_;
};

#ifdef _WIN32

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

inline unsigned long current_process_id() {
    return static_cast<unsigned long>(GetCurrentProcessId());
}

inline bool exists(const path& target) {
    return GetFileAttributesW(wide_from_utf8(target.string()).c_str()) != INVALID_FILE_ATTRIBUTES;
}

inline bool is_directory(const path& target) {
    const DWORD attributes = GetFileAttributesW(wide_from_utf8(target.string()).c_str());
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
    if (!CreateDirectoryW(wide_from_utf8(target.string()).c_str(), NULL) && GetLastError() != ERROR_ALREADY_EXISTS) {
        throw_last_error("CreateDirectoryW", target);
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
    const DWORD attributes = GetFileAttributesW(wide_from_utf8(target.string()).c_str());
    if (attributes == INVALID_FILE_ATTRIBUTES) {
        return;
    }

    if ((attributes & FILE_ATTRIBUTE_DIRECTORY) == 0) {
        if (!DeleteFileW(wide_from_utf8(target.string()).c_str())) {
            throw_last_error("DeleteFileW", target);
        }
        return;
    }

    const path search = target / "*";
    WIN32_FIND_DATAW data;
    HANDLE handle = FindFirstFileW(wide_from_utf8(search.string()).c_str(), &data);
    if (handle != INVALID_HANDLE_VALUE) {
        do {
            const std::string name = utf8_from_wide(data.cFileName);
            if (name == "." || name == "..") {
                continue;
            }
            remove_all(target / name);
        } while (FindNextFileW(handle, &data));
        FindClose(handle);
    }

    if (!RemoveDirectoryW(wide_from_utf8(target.string()).c_str())) {
        throw_last_error("RemoveDirectoryW", target);
    }
}

#else

inline void throw_errno(const std::string& operation, const path& target, int error) {
    throw std::runtime_error(operation + " failed for `" + target.string() + "`: " + std::strerror(error));
}

inline path temp_directory_path() {
    const char* temp = std::getenv("TMPDIR");
    if (temp == NULL || temp[0] == '\0') {
        temp = "/tmp";
    }
    return path(temp);
}

inline unsigned long current_process_id() {
    return static_cast<unsigned long>(getpid());
}

inline bool exists(const path& target) {
    struct stat info;
    return ::stat(target.c_str(), &info) == 0;
}

inline bool is_directory(const path& target) {
    struct stat info;
    return ::stat(target.c_str(), &info) == 0 && S_ISDIR(info.st_mode);
}

inline void create_directory_if_missing(const path& target) {
    if (target.string().empty() || exists(target)) {
        return;
    }
    if (::mkdir(target.c_str(), 0777) != 0 && errno != EEXIST) {
        throw_errno("mkdir", target, errno);
    }
}

inline void create_directories(const path& target) {
    const std::string text = target.string();
    if (text.empty() || exists(target)) {
        return;
    }

    std::size_t start = 0;
    while (start < text.size() && text[start] == '/') {
        ++start;
    }

    for (std::size_t index = start; index <= text.size(); ++index) {
        if (index != text.size() && text[index] != '/') {
            continue;
        }
        const std::string prefix = text.substr(0, index);
        if (!prefix.empty()) {
            create_directory_if_missing(path(prefix));
        }
    }
}

inline void remove_all(const path& target) {
    struct stat info;
    if (::lstat(target.c_str(), &info) != 0) {
        if (errno == ENOENT) {
            return;
        }
        throw_errno("lstat", target, errno);
    }

    if (!S_ISDIR(info.st_mode) || S_ISLNK(info.st_mode)) {
        if (::unlink(target.c_str()) != 0) {
            throw_errno("unlink", target, errno);
        }
        return;
    }

    DIR* dir = ::opendir(target.c_str());
    if (dir == NULL) {
        throw_errno("opendir", target, errno);
    }

    for (;;) {
        errno = 0;
        dirent* entry = ::readdir(dir);
        if (entry == NULL) {
            if (errno != 0) {
                const int error = errno;
                ::closedir(dir);
                throw_errno("readdir", target, error);
            }
            break;
        }

        const std::string name(entry->d_name);
        if (name == "." || name == "..") {
            continue;
        }
        remove_all(target / name);
    }

    if (::closedir(dir) != 0) {
        throw_errno("closedir", target, errno);
    }
    if (::rmdir(target.c_str()) != 0) {
        throw_errno("rmdir", target, errno);
    }
}

inline void create_symlink(const path& target, const path& link) {
    if (::symlink(target.c_str(), link.c_str()) != 0) {
        throw_errno("symlink", link, errno);
    }
}

inline path read_symlink(const path& link) {
    std::vector<char> buffer(256U);
    for (;;) {
        const ssize_t length = ::readlink(link.c_str(), &buffer[0], buffer.size());
        if (length < 0) {
            throw_errno("readlink", link, errno);
        }
        if (static_cast<std::size_t>(length) < buffer.size()) {
            return path(std::string(&buffer[0], static_cast<std::size_t>(length)));
        }
        buffer.resize(buffer.size() * 2U);
    }
}

inline file_status status(const path& target) {
    struct stat info;
    if (::stat(target.c_str(), &info) != 0) {
        throw_errno("stat", target, errno);
    }
    return file_status(static_cast<perms>(static_cast<unsigned>(info.st_mode) & 07777U));
}

inline void permissions(const path& target, perms mode, perm_options options) {
    unsigned next_mode = static_cast<unsigned>(mode);
    if (options == perm_options::add) {
        next_mode |= static_cast<unsigned>(status(target).permissions());
    }
    if (::chmod(target.c_str(), static_cast<mode_t>(next_mode)) != 0) {
        throw_errno("chmod", target, errno);
    }
}

class directory_entry {
public:
    directory_entry() {}
    explicit directory_entry(const test_fs::path& value) : path_(value) {}

    const test_fs::path& path() const { return path_; }

private:
    test_fs::path path_;
};

class directory_iterator {
public:
    directory_iterator() : dir_(NULL) {}

    explicit directory_iterator(const test_fs::path& root) : dir_(NULL), root_(root) {
        dir_ = ::opendir(root.c_str());
        if (dir_ == NULL) {
            throw_errno("opendir", root, errno);
        }
        advance();
    }

    ~directory_iterator() {
        if (dir_ != NULL) {
            ::closedir(dir_);
        }
    }

    const directory_entry& operator*() const { return current_; }

    const directory_entry* operator->() const { return &current_; }

    directory_iterator& operator++() {
        advance();
        return *this;
    }

    bool operator==(const directory_iterator& other) const {
        return dir_ == NULL && other.dir_ == NULL;
    }

    bool operator!=(const directory_iterator& other) const {
        return !(*this == other);
    }

private:
    directory_iterator(const directory_iterator&);
    directory_iterator& operator=(const directory_iterator&);

    void advance() {
        if (dir_ == NULL) {
            return;
        }

        for (;;) {
            errno = 0;
            dirent* entry = ::readdir(dir_);
            if (entry == NULL) {
                if (errno != 0) {
                    throw_errno("readdir", root_, errno);
                }
                ::closedir(dir_);
                dir_ = NULL;
                current_ = directory_entry();
                return;
            }

            const std::string name(entry->d_name);
            if (name == "." || name == "..") {
                continue;
            }
            current_ = directory_entry(root_ / name);
            return;
        }
    }

    DIR* dir_;
    test_fs::path root_;
    directory_entry current_;
};

#endif

inline path unique_test_root(const std::string& name) {
    return temp_directory_path() / (name + "-" + std::to_string(current_process_id()));
}

} // namespace test_fs

#endif
