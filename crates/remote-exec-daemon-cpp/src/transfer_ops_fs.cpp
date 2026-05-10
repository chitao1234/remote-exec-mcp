#include <algorithm>
#include <cerrno>
#include <cctype>
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

#include "rpc_failures.h"
#include "transfer_ops_internal.h"

namespace transfer_ops_internal {

bool is_absolute_path(const std::string& path) {
#ifdef _WIN32
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 &&
            path[1] == ':' && (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
#else
    return !path.empty() && path[0] == '/';
#endif
}

namespace {

std::string parent_directory(const std::string& path) {
    const std::size_t slash = path.find_last_of("/\\");
    if (slash == std::string::npos) {
        return "";
    }
    return path.substr(0, slash);
}

bool stat_path_no_follow(const std::string& path, struct stat* st) {
#ifdef _WIN32
    return stat(path.c_str(), st) == 0;
#else
    return lstat(path.c_str(), st) == 0;
#endif
}

bool stat_is_regular_file(const struct stat& st) {
    return (st.st_mode & S_IFMT) == S_IFREG;
}

bool stat_is_directory(const struct stat& st) {
    return (st.st_mode & S_IFMT) == S_IFDIR;
}

char os_separator() {
#ifdef _WIN32
    return '\\';
#else
    return '/';
#endif
}

void remove_existing_path(const std::string& path) {
    if (!path_exists(path)) {
        return;
    }

    if (is_directory(path)) {
        const std::vector<DirectoryEntry> entries = list_directory_entries(path);
        for (std::size_t i = 0; i < entries.size(); ++i) {
            remove_existing_path(join_path(path, entries[i].name));
        }
#ifdef _WIN32
        if (_rmdir(path.c_str()) != 0) {
#else
        if (rmdir(path.c_str()) != 0) {
#endif
            throw std::runtime_error("unable to remove existing directory " + path);
        }
        return;
    }

    if (std::remove(path.c_str()) != 0) {
        throw std::runtime_error("unable to remove existing file " + path);
    }
}

}  // namespace

bool is_symlink_path(const std::string& path) {
#ifdef _WIN32
    const DWORD attributes = GetFileAttributesA(path.c_str());
    return attributes != INVALID_FILE_ATTRIBUTES &&
           (attributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
#else
    struct stat st;
    return lstat(path.c_str(), &st) == 0 && S_ISLNK(st.st_mode);
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
    return stat(path.c_str(), &st) == 0 && stat_is_regular_file(st);
}

bool is_directory(const std::string& path) {
    struct stat st;
    return stat_path_no_follow(path, &st) && stat_is_directory(st);
}

bool is_directory_follow(const std::string& path) {
    struct stat st;
    return stat(path.c_str(), &st) == 0 && stat_is_directory(st);
}

std::string join_path(const std::string& base, const std::string& child) {
    if (base.empty()) {
        return child;
    }
    std::string joined = base;
    if (!joined.empty() && joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(os_separator());
    }
    joined += child;
    return joined;
}

void make_directory_if_missing(const std::string& path) {
    if (path.empty() || is_directory(path)) {
        return;
    }
#ifdef _WIN32
    if (_mkdir(path.c_str()) != 0 && errno != EEXIST) {
#else
    if (mkdir(path.c_str(), 0777) != 0 && errno != EEXIST) {
#endif
        throw std::runtime_error("unable to create directory " + path);
    }
}

void ensure_parent_directory(const std::string& path, bool create_parent) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }
    if (!create_parent) {
        if (!is_directory(parent)) {
            throw TransferFailure(
                TransferRpcCode::ParentMissing,
                "destination parent does not exist"
            );
        }
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

void ensure_not_existing_symlink(const std::string& path) {
    if (path_exists(path) && is_symlink_path(path)) {
        throw TransferFailure(
            TransferRpcCode::DestinationUnsupported,
            "destination path contains unsupported symlink"
        );
    }
}

void write_symlink(const std::string& target, const std::string& path) {
#ifdef _WIN32
    (void)target;
    throw TransferFailure(
        TransferRpcCode::SourceUnsupported,
        "archive contains unsupported symlink " + path
    );
#else
    ensure_parent_directory(path, true);
    if (path_exists(path)) {
        if (is_directory(path)) {
            throw TransferFailure(
                TransferRpcCode::DestinationUnsupported,
                "destination path is a directory"
            );
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
    std::string pattern = path;
    if (!pattern.empty() && pattern[pattern.size() - 1] != '\\' && pattern[pattern.size() - 1] != '/') {
        pattern.push_back('\\');
    }
    pattern.push_back('*');

    WIN32_FIND_DATAA find_data;
    HANDLE handle = FindFirstFileA(pattern.c_str(), &find_data);
    if (handle == INVALID_HANDLE_VALUE) {
        throw std::runtime_error("unable to read directory " + path);
    }

    do {
        const std::string name(find_data.cFileName);
        if (name == "." || name == "..") {
            continue;
        }
        const bool entry_is_symlink =
            (find_data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
        const bool entry_is_directory =
            !entry_is_symlink &&
            (find_data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        entries.push_back(
            DirectoryEntry{name, entry_is_directory, !entry_is_directory && !entry_is_symlink, entry_is_symlink}
        );
    } while (FindNextFileA(handle, &find_data) != 0);

    const DWORD last_error = GetLastError();
    FindClose(handle);
    if (last_error != ERROR_NO_MORE_FILES) {
        throw std::runtime_error("unable to read directory " + path);
    }
#else
    DIR* dir = opendir(path.c_str());
    if (dir == NULL) {
        throw std::runtime_error("unable to read directory " + path);
    }

    dirent* entry = NULL;
    while ((entry = readdir(dir)) != NULL) {
        const std::string name(entry->d_name);
        if (name == "." || name == "..") {
            continue;
        }
        const std::string child = join_path(path, name);
        struct stat st;
        if (!stat_path_no_follow(child, &st)) {
            closedir(dir);
            throw std::runtime_error("unable to stat path " + child);
        }
        entries.push_back(
            DirectoryEntry{
                name,
                S_ISDIR(st.st_mode),
                S_ISREG(st.st_mode),
                S_ISLNK(st.st_mode)
            }
        );
    }
    closedir(dir);
#endif

    std::sort(
        entries.begin(),
        entries.end(),
        [](const DirectoryEntry& left, const DirectoryEntry& right) {
            return left.name < right.name;
        }
    );
    return entries;
}

bool prepare_destination_path(
    const std::string& absolute_path,
    TransferSourceType source_type,
    const std::string& overwrite_mode,
    bool create_parent
) {
    const bool existed = path_exists(absolute_path);
    if (overwrite_mode != "fail" && overwrite_mode != "merge" && overwrite_mode != "replace") {
        throw TransferFailure(
            TransferRpcCode::TransferFailed,
            "unsupported transfer overwrite mode"
        );
    }
    if (existed && overwrite_mode == "fail") {
        throw TransferFailure(
            TransferRpcCode::DestinationExists,
            "destination path already exists"
        );
    }

    ensure_parent_directory(absolute_path, create_parent);

    if (existed && overwrite_mode == "merge") {
        ensure_not_existing_symlink(absolute_path);
        if (source_type == TransferSourceType::File) {
            if (is_directory(absolute_path)) {
                throw TransferFailure(
                    TransferRpcCode::DestinationUnsupported,
                    "destination path is a directory"
                );
            }
            if (!is_regular_file(absolute_path)) {
                throw TransferFailure(
                    TransferRpcCode::DestinationUnsupported,
                    "destination path is not a regular file"
                );
            }
        } else if (
            source_type == TransferSourceType::Directory ||
            source_type == TransferSourceType::Multiple
        ) {
            if (!is_directory(absolute_path)) {
                throw TransferFailure(
                    TransferRpcCode::DestinationUnsupported,
                    "destination path is not a directory"
                );
            }
        } else {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "unsupported transfer source type"
            );
        }
    }

    if (existed && overwrite_mode == "replace") {
        remove_existing_path(absolute_path);
    }

    return existed && overwrite_mode == "replace";
}

}  // namespace transfer_ops_internal
