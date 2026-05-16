#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cstdio>
#include <stdexcept>
#include <string>
#include <vector>

#ifdef _WIN32
#include <direct.h>
#include <sys/stat.h>
#include <windows.h>
#else
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "path_utils.h"
#include "rpc_failures.h"
#include "transfer_ops_internal.h"

namespace transfer_ops_internal {

bool is_absolute_path(const std::string& path) {
#ifdef _WIN32
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 && path[1] == ':' &&
            (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
#else
    return !path.empty() && path[0] == '/';
#endif
}

namespace {

bool stat_path_no_follow(const std::string& path, struct stat* st) {
    return path_utils::lstat_path(path, st);
}

bool stat_is_regular_file(const struct stat& st) {
    return (st.st_mode & S_IFMT) == S_IFREG;
}

bool stat_is_directory(const struct stat& st) {
    return (st.st_mode & S_IFMT) == S_IFDIR;
}

#ifdef _WIN32
class ScopedFindHandle {
  public:
    explicit ScopedFindHandle(HANDLE handle) : handle_(handle) {
    }

    ~ScopedFindHandle() {
        if (handle_ != INVALID_HANDLE_VALUE) {
            FindClose(handle_);
        }
    }

    HANDLE get() const {
        return handle_;
    }

    bool valid() const {
        return handle_ != INVALID_HANDLE_VALUE;
    }

  private:
    HANDLE handle_;
};
#else
class ScopedDirHandle {
  public:
    explicit ScopedDirHandle(DIR* dir) : dir_(dir) {
    }

    ~ScopedDirHandle() {
        if (dir_ != nullptr) {
            closedir(dir_);
        }
    }

    DIR* get() const {
        return dir_;
    }

  private:
    DIR* dir_;
};
#endif

static const std::size_t MAX_REMOVE_DEPTH = 256;

void remove_existing_path_recursive(const std::string& path, std::size_t depth) {
    if (depth > MAX_REMOVE_DEPTH) {
        throw std::runtime_error("remove_existing_path exceeded maximum depth of " +
                                 std::to_string(MAX_REMOVE_DEPTH));
    }

    if (!path_exists(path)) {
        return;
    }

    if (is_directory(path)) {
        const std::vector<DirectoryEntry> entries = list_directory_entries(path);
        for (std::size_t i = 0; i < entries.size(); ++i) {
            remove_existing_path_recursive(join_path(path, entries[i].name), depth + 1);
        }
#ifdef _WIN32
        if (!path_utils::remove_directory(path)) {
#else
        if (!path_utils::remove_directory(path)) {
#endif
            throw std::runtime_error("unable to remove existing directory " + path);
        }
        return;
    }

    if (!path_utils::remove_path(path)) {
        throw std::runtime_error("unable to remove existing file " + path);
    }
}

void remove_existing_path(const std::string& path) {
    remove_existing_path_recursive(path, 0);
}

} // namespace

bool is_symlink_path(const std::string& path) {
#ifdef _WIN32
    const DWORD attributes = GetFileAttributesW(path_utils::wide_from_utf8(path).c_str());
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
#else
    struct stat st;
    return path_utils::lstat_path(path, &st) && S_ISLNK(st.st_mode);
#endif
}

bool path_exists(const std::string& path) {
    struct stat st;
    return stat_path_no_follow(path, &st);
}

bool is_regular_file(const std::string& path) {
    struct stat st;
    return stat_path_no_follow(path, &st) && stat_is_regular_file(st);
}

bool is_regular_file_follow(const std::string& path) {
    struct stat st;
    return path_utils::stat_path(path, &st) && stat_is_regular_file(st);
}

bool is_directory(const std::string& path) {
    struct stat st;
    return stat_path_no_follow(path, &st) && stat_is_directory(st);
}

bool is_directory_follow(const std::string& path) {
    struct stat st;
    return path_utils::stat_path(path, &st) && stat_is_directory(st);
}

std::string join_path(const std::string& base, const std::string& child) {
    return path_utils::join_path(base, child);
}

void make_directory_if_missing(const std::string& path) {
    if (path.empty() || is_directory(path)) {
        return;
    }
    path_utils::make_directory_if_missing(path);
}

void ensure_parent_directory(const std::string& path, bool create_parent) {
    const std::string parent = path_utils::parent_directory(path);
    if (parent.empty()) {
        return;
    }
    if (!create_parent) {
        if (!is_directory(parent)) {
            throw TransferFailure(TransferRpcCode::ParentMissing, "destination parent does not exist");
        }
        return;
    }

    path_utils::create_parent_directories(path);
}

void ensure_not_existing_symlink(const std::string& path) {
    if (path_exists(path) && is_symlink_path(path)) {
        throw TransferFailure(TransferRpcCode::DestinationUnsupported, "destination path contains unsupported symlink");
    }
}

void write_symlink(const std::string& target, const std::string& path) {
#ifdef _WIN32
    (void)target;
    throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive contains unsupported symlink " + path);
#else
    ensure_parent_directory(path, true);
    if (path_exists(path)) {
        if (is_directory(path)) {
            throw TransferFailure(TransferRpcCode::DestinationUnsupported, "destination path is a directory");
        }
        if (std::remove(path.c_str()) != 0) {
            throw std::runtime_error("unable to remove existing file " + path);
        }
    }
    if (symlink(target.c_str(), path.c_str()) != 0) {
        throw std::runtime_error("unable to create symlink " + path);
    }
#endif
}

std::vector<DirectoryEntry> list_directory_entries(const std::string& path) {
    std::vector<DirectoryEntry> entries;
#ifdef _WIN32
    std::wstring pattern = path_utils::wide_from_utf8(path);
    if (!pattern.empty() && pattern[pattern.size() - 1] != '\\' && pattern[pattern.size() - 1] != '/') {
        pattern.push_back(L'\\');
    }
    pattern.push_back(L'*');

    WIN32_FIND_DATAW find_data;
    ScopedFindHandle handle(FindFirstFileW(pattern.c_str(), &find_data));
    if (!handle.valid()) {
        throw std::runtime_error("unable to read directory " + path);
    }

    do {
        const std::string name = path_utils::utf8_from_wide(find_data.cFileName);
        if (name == "." || name == "..") {
            continue;
        }
        const bool entry_is_symlink = (find_data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
        const bool entry_is_directory =
            !entry_is_symlink && (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        entries.push_back(
            DirectoryEntry{name, entry_is_directory, !entry_is_directory && !entry_is_symlink, entry_is_symlink});
    } while (FindNextFileW(handle.get(), &find_data) != 0);

    const DWORD last_error = GetLastError();
    if (last_error != ERROR_NO_MORE_FILES) {
        throw std::runtime_error("unable to read directory " + path);
    }
#else
    ScopedDirHandle dir(opendir(path.c_str()));
    if (dir.get() == nullptr) {
        throw std::runtime_error("unable to read directory " + path);
    }

    dirent* entry = nullptr;
    while ((entry = readdir(dir.get())) != nullptr) {
        const std::string name(entry->d_name);
        if (name == "." || name == "..") {
            continue;
        }
        const std::string child = join_path(path, name);
        struct stat st;
        if (!stat_path_no_follow(child, &st)) {
            throw std::runtime_error("unable to stat path " + child);
        }
        entries.push_back(DirectoryEntry{name, S_ISDIR(st.st_mode), S_ISREG(st.st_mode), S_ISLNK(st.st_mode)});
    }
#endif

    std::sort(entries.begin(), entries.end(), [](const DirectoryEntry& left, const DirectoryEntry& right) {
        return left.name < right.name;
    });
    return entries;
}

bool prepare_destination_path(const std::string& absolute_path,
                              TransferSourceType source_type,
                              TransferOverwrite overwrite,
                              bool create_parent) {
    const bool existed = path_exists(absolute_path);
    if (existed && overwrite == TransferOverwrite::Fail) {
        throw TransferFailure(TransferRpcCode::DestinationExists, "destination path already exists");
    }

    ensure_parent_directory(absolute_path, create_parent);

    if (existed && overwrite == TransferOverwrite::Merge) {
        ensure_not_existing_symlink(absolute_path);
        if (source_type == TransferSourceType::File) {
            if (is_directory(absolute_path)) {
                throw TransferFailure(TransferRpcCode::DestinationUnsupported, "destination path is a directory");
            }
            if (!is_regular_file(absolute_path)) {
                throw TransferFailure(TransferRpcCode::DestinationUnsupported,
                                      "destination path is not a regular file");
            }
        } else if (source_type == TransferSourceType::Directory || source_type == TransferSourceType::Multiple) {
            if (!is_directory(absolute_path)) {
                throw TransferFailure(TransferRpcCode::DestinationUnsupported, "destination path is not a directory");
            }
        } else {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "unsupported transfer source type");
        }
    }

    if (existed && overwrite == TransferOverwrite::Replace) {
        remove_existing_path(absolute_path);
    }

    return existed && overwrite == TransferOverwrite::Replace;
}

} // namespace transfer_ops_internal
