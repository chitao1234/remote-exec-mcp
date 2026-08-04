#include "platform/path_utils.h"

#include <algorithm>
#include <cctype>
#include <cerrno>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include "platform/win32_native_string.h"
#include <direct.h>
#include <io.h>
#include <windows.h>
#else
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#ifndef _WIN32
#include "platform/posix_eintr.h"
#endif

namespace path_utils {

#ifdef _WIN32
namespace {

const unsigned int RENAME_RETRY_ATTEMPTS = 8U;
const unsigned long RENAME_RETRY_MAX_DELAY_MS = 16UL;

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
    return error == ERROR_ACCESS_DENIED || error == ERROR_SHARING_VIOLATION
           || error == ERROR_LOCK_VIOLATION || error == ERROR_ALREADY_EXISTS
           || error == ERROR_FILE_EXISTS;
}

#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
BOOL move_file_replace_ansi(const char* source, const char* destination) {
    if (MoveFileA(source, destination) != 0) {
        return TRUE;
    }

    const DWORD move_error = GetLastError();
    if (move_error != ERROR_ALREADY_EXISTS && move_error != ERROR_FILE_EXISTS) {
        SetLastError(move_error);
        return FALSE;
    }

    if (DeleteFileA(destination) == 0) {
        const DWORD delete_error = GetLastError();
        if (delete_error != ERROR_FILE_NOT_FOUND && delete_error != ERROR_PATH_NOT_FOUND) {
            SetLastError(delete_error);
            return FALSE;
        }
    }

    return MoveFileA(source, destination);
}
#endif

bool is_windows_separator(remote_exec_win32::NativeChar ch) {
    return ch == remote_exec_win32::native_char('\\') || ch == remote_exec_win32::native_char('/');
}

bool has_find_wildcard(const remote_exec_win32::NativeString& path) {
    std::size_t start = 0U;
    if (path.size() >= 4U && is_windows_separator(path[0]) && is_windows_separator(path[1])
        && (path[2] == remote_exec_win32::native_char('?')
            || path[2] == remote_exec_win32::native_char('.'))
        && is_windows_separator(path[3])) {
        start = 4U;
    }

    for (std::size_t i = start; i < path.size(); ++i) {
        if (path[i] == remote_exec_win32::native_char('*')
            || path[i] == remote_exec_win32::native_char('?')) {
            return true;
        }
    }
    return false;
}

void fill_metadata_from_win32_attributes(
    PathMetadata* metadata,
    DWORD attributes,
    DWORD file_size_high,
    DWORD file_size_low
) {
    *metadata = PathMetadata();
    metadata->exists = true;
    metadata->is_symlink = (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    metadata->is_directory = !metadata->is_symlink && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
    metadata->is_regular_file = !metadata->is_directory && !metadata->is_symlink;
    metadata->size = (static_cast<std::uint64_t>(file_size_high) << 32) | file_size_low;
    metadata->mode_bits = (attributes & FILE_ATTRIBUTE_READONLY) != 0 ? 0444U : 0666U;
    metadata->has_mode_bits = true;
}

class ScopedFindHandle {
public:
    explicit ScopedFindHandle(HANDLE handle) : handle_(handle) {}

    ~ScopedFindHandle() {
        if (handle_ != INVALID_HANDLE_VALUE) {
            FindClose(handle_);
        }
    }

    HANDLE get() const { return handle_; }

    bool valid() const { return handle_ != INVALID_HANDLE_VALUE; }

private:
    HANDLE handle_;
};

} // namespace
#else
namespace {

const std::size_t MAX_SYMLINK_TARGET_BYTES = 1024U * 1024U;

class ScopedDirHandle {
public:
    explicit ScopedDirHandle(DIR* dir) : dir_(dir) {}

    ~ScopedDirHandle() {
        if (dir_ != nullptr) {
            closedir(dir_);
        }
    }

    DIR* get() const { return dir_; }

private:
    DIR* dir_;
};

void fill_metadata_from_stat(PathMetadata* metadata, const struct stat& st) {
    *metadata = PathMetadata();
    metadata->exists = true;
    metadata->is_regular_file = S_ISREG(st.st_mode);
    metadata->is_directory = S_ISDIR(st.st_mode);
    metadata->is_symlink = S_ISLNK(st.st_mode);
    metadata->size = st.st_size < 0 ? 0U : static_cast<std::uint64_t>(st.st_size);
    metadata->mode_bits = static_cast<unsigned int>(st.st_mode & 07777U);
    metadata->has_mode_bits = true;
}

} // namespace
#endif

char native_separator() {
#ifdef _WIN32
    return '\\';
#else
    return '/';
#endif
}

bool parse_absolute_path_prefix(
    PathStyle style,
    const std::string& path,
    AbsolutePathPrefix* prefix
) {
    if (prefix == nullptr) {
        return false;
    }
    prefix->value.clear();
    prefix->start = 0;

    if (style == PATH_STYLE_POSIX) {
        if (!path.empty() && path[0] == '/') {
            prefix->value = "/";
            prefix->start = 1;
            return true;
        }
        return false;
    }

    if (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 && path[1] == ':'
        && (path[2] == '\\' || path[2] == '/')) {
        prefix->value = path.substr(0, 2);
        prefix->value.push_back('\\');
        prefix->start = 3;
        return true;
    }
    if (path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0) {
        prefix->value = "\\\\";
        prefix->start = 2;
        return true;
    }
    return false;
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
    const remote_exec_win32::NativeString native =
        remote_exec_win32::native_from_utf8(path, "mkdir");
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    if (_mkdir(native.c_str()) != 0 && errno != EEXIST) {
#else
    if (_wmkdir(native.c_str()) != 0 && errno != EEXIST) {
#endif
#else
    if (posix_eintr::retry<int>([&]() { return mkdir(path.c_str(), 0777); }) != 0
        && errno != EEXIST) {
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
    const remote_exec_win32::NativeString native_path =
        remote_exec_win32::native_from_utf8(path, "fopen");
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return std::fopen(native_path.c_str(), mode);
#else
    const remote_exec_win32::NativeString native_mode = remote_exec_win32::native_from_ascii(mode);
    return _wfopen(native_path.c_str(), native_mode.c_str());
#endif
#else
    return posix_eintr::retry_null<FILE*>([&]() { return std::fopen(path.c_str(), mode); });
#endif
}

bool path_metadata(const std::string& path, PathMetadata* metadata) {
#ifdef _WIN32
    const remote_exec_win32::NativeString native_path =
        remote_exec_win32::native_from_utf8(path, "FindFirstFile");
    if (has_find_wildcard(native_path)) {
        errno = last_error_to_errno(ERROR_INVALID_NAME);
        return false;
    }

    remote_exec_win32::NativeFindData data;
    ScopedFindHandle handle(remote_exec_win32::find_first_file_native(native_path.c_str(), &data));
    if (handle.valid()) {
        fill_metadata_from_win32_attributes(
            metadata,
            data.dwFileAttributes,
            data.nFileSizeHigh,
            data.nFileSizeLow
        );
        return true;
    }

    const DWORD find_error = GetLastError();
    const DWORD attributes = remote_exec_win32::get_file_attributes_native(native_path.c_str());
    if (attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY) != 0) {
        fill_metadata_from_win32_attributes(metadata, attributes, 0U, 0U);
        return true;
    }

    errno = last_error_to_errno(find_error);
    return false;
#else
    struct stat st;
    if (posix_eintr::retry<int>([&]() { return stat(path.c_str(), &st); }) != 0) {
        return false;
    }
    fill_metadata_from_stat(metadata, st);
    return true;
#endif
}

bool path_metadata_no_follow(const std::string& path, PathMetadata* metadata) {
#ifdef _WIN32
    return path_metadata(path, metadata);
#else
    struct stat st;
    if (posix_eintr::retry<int>([&]() { return lstat(path.c_str(), &st); }) != 0) {
        return false;
    }
    fill_metadata_from_stat(metadata, st);
    return true;
#endif
}

bool file_identity(const std::string& path, FileIdentity* identity) {
    if (identity == nullptr) {
        errno = EINVAL;
        return false;
    }
    *identity = FileIdentity();
#ifdef _WIN32
    (void)path;
    return false;
#else
    struct stat st;
    if (posix_eintr::retry<int>([&]() { return stat(path.c_str(), &st); }) != 0) {
        return false;
    }
    identity->valid = true;
    identity->device = static_cast<unsigned long long>(st.st_dev);
    identity->file = static_cast<unsigned long long>(st.st_ino);
    return true;
#endif
}

bool remove_path(const std::string& path) {
#ifdef _WIN32
    const remote_exec_win32::NativeString native =
        remote_exec_win32::native_from_utf8(path, "remove");
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return std::remove(native.c_str()) == 0;
#else
    return _wremove(native.c_str()) == 0;
#endif
#else
    return posix_eintr::retry<int>([&]() { return std::remove(path.c_str()); }) == 0;
#endif
}

bool remove_directory(const std::string& path) {
#ifdef _WIN32
    const remote_exec_win32::NativeString native =
        remote_exec_win32::native_from_utf8(path, "rmdir");
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return _rmdir(native.c_str()) == 0;
#else
    return _wrmdir(native.c_str()) == 0;
#endif
#else
    return posix_eintr::retry<int>([&]() { return rmdir(path.c_str()); }) == 0;
#endif
}

bool rename_path(const std::string& source, const std::string& destination) {
#ifdef _WIN32
    const remote_exec_win32::NativeString native_source = remote_exec_win32::native_from_utf8(
        source,
        remote_exec_win32::native_api_name("MoveFile").c_str()
    );
    const remote_exec_win32::NativeString native_destination = remote_exec_win32::native_from_utf8(
        destination,
        remote_exec_win32::native_api_name("MoveFile").c_str()
    );
    DWORD last_error = ERROR_SUCCESS;
    unsigned long retry_delay_ms = 1UL;

    for (unsigned int attempt = 0U; attempt < RENAME_RETRY_ATTEMPTS; ++attempt) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
        const BOOL moved =
            move_file_replace_ansi(native_source.c_str(), native_destination.c_str());
#else
        const BOOL moved = MoveFileExW(
            native_source.c_str(),
            native_destination.c_str(),
            MOVEFILE_REPLACE_EXISTING
        );
#endif
        if (moved != 0) {
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
    return posix_eintr::retry<int>([&]() {
               return std::rename(source.c_str(), destination.c_str());
           })
           == 0;
#endif
}

bool set_path_mode(const std::string& path, unsigned int mode) {
#ifdef _WIN32
    const remote_exec_win32::NativeString native =
        remote_exec_win32::native_from_utf8(path, "chmod");
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    return _chmod(native.c_str(), static_cast<int>(mode)) == 0;
#else
    return _wchmod(native.c_str(), static_cast<int>(mode)) == 0;
#endif
#else
    return posix_eintr::retry<int>([&]() { return chmod(path.c_str(), static_cast<mode_t>(mode)); })
           == 0;
#endif
}

bool add_executable_bits(FILE* file) {
    if (file == nullptr) {
        errno = EINVAL;
        return false;
    }
#ifdef _WIN32
    (void)file;
    return true;
#else
    const int fd = fileno(file);
    if (fd < 0) {
        return false;
    }
    struct stat st;
    if (posix_eintr::retry<int>([&]() { return fstat(fd, &st); }) != 0) {
        return false;
    }
    return posix_eintr::retry<int>([&]() { return fchmod(fd, st.st_mode | 0111); }) == 0;
#endif
}

bool read_symlink_target(const std::string& path, std::string* target) {
    if (target == nullptr) {
        errno = EINVAL;
        return false;
    }
#ifdef _WIN32
    (void)path;
#ifdef ENOSYS
    errno = ENOSYS;
#else
    errno = EINVAL;
#endif
    return false;
#else
    std::size_t capacity = 4096U;
    while (capacity <= MAX_SYMLINK_TARGET_BYTES) {
        std::vector<char> buffer(capacity);
        const ssize_t target_len = posix_eintr::retry<ssize_t>([&]() {
            return readlink(path.c_str(), buffer.data(), buffer.size());
        });
        if (target_len < 0) {
            return false;
        }
        if (static_cast<std::size_t>(target_len) < buffer.size()) {
            target->assign(buffer.data(), static_cast<std::size_t>(target_len));
            return true;
        }
        if (capacity > MAX_SYMLINK_TARGET_BYTES / 2U) {
            break;
        }
        capacity *= 2U;
    }
    errno = ENAMETOOLONG;
    return false;
#endif
}

bool create_symlink(const std::string& target, const std::string& path) {
#ifdef _WIN32
    (void)target;
    (void)path;
#ifdef ENOSYS
    errno = ENOSYS;
#else
    errno = EINVAL;
#endif
    return false;
#else
    return posix_eintr::retry<int>([&]() { return symlink(target.c_str(), path.c_str()); }) == 0;
#endif
}

std::vector<DirectoryEntryInfo> read_directory_entries(const std::string& path) {
    std::vector<DirectoryEntryInfo> entries;
#ifdef _WIN32
    remote_exec_win32::NativeString pattern =
        remote_exec_win32::native_from_utf8(path, "FindFirstFile");
    if (!pattern.empty() && pattern[pattern.size() - 1] != remote_exec_win32::native_char('\\')
        && pattern[pattern.size() - 1] != remote_exec_win32::native_char('/')) {
        pattern.push_back(remote_exec_win32::native_char('\\'));
    }
    pattern.push_back(remote_exec_win32::native_char('*'));

    remote_exec_win32::NativeFindData find_data;
    ScopedFindHandle handle(remote_exec_win32::find_first_file_native(pattern.c_str(), &find_data));
    if (!handle.valid()) {
        errno = last_error_to_errno(GetLastError());
        throw std::runtime_error("unable to read directory " + path);
    }

    do {
        const std::string name =
            remote_exec_win32::utf8_from_native(find_data.cFileName, "FindFirstFile");
        if (name == "." || name == "..") {
            continue;
        }
        const bool entry_is_symlink =
            (find_data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
        const bool entry_is_directory =
            !entry_is_symlink && (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        entries.push_back(DirectoryEntryInfo{
            name,
            entry_is_directory,
            !entry_is_directory && !entry_is_symlink,
            entry_is_symlink
        });
    } while (remote_exec_win32::find_next_file_native(handle.get(), &find_data) != 0);

    const DWORD last_error = GetLastError();
    if (last_error != ERROR_NO_MORE_FILES) {
        errno = last_error_to_errno(last_error);
        throw std::runtime_error("unable to read directory " + path);
    }
#else
    ScopedDirHandle dir(posix_eintr::retry_null<DIR*>([&]() { return opendir(path.c_str()); }));
    if (dir.get() == nullptr) {
        throw std::runtime_error("unable to read directory " + path);
    }

    dirent* entry = nullptr;
    errno = 0;
    while ((entry = posix_eintr::retry_null<dirent*>([&]() {
                errno = 0;
                return readdir(dir.get());
            }))
           != nullptr) {
        const std::string name(entry->d_name);
        if (name == "." || name == "..") {
            continue;
        }
        const std::string child = join_path(path, name);
        PathMetadata metadata;
        if (!path_metadata_no_follow(child, &metadata)) {
            throw std::runtime_error("unable to stat path " + child);
        }
        entries.push_back(DirectoryEntryInfo{
            name,
            metadata.is_directory,
            metadata.is_regular_file,
            metadata.is_symlink
        });
        errno = 0;
    }
    if (errno != 0) {
        throw std::runtime_error("unable to read directory " + path);
    }
#endif

    std::sort(
        entries.begin(),
        entries.end(),
        [](const DirectoryEntryInfo& left, const DirectoryEntryInfo& right) {
            return left.name < right.name;
        }
    );
    return entries;
}

} // namespace path_utils
