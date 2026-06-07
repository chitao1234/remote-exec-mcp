#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cstdio>
#include <stdexcept>
#include <string>
#include <vector>

#include "platform/path_utils.h"
#include "rpc/rpc_failures.h"
#include "transfer_filesystem.h"

namespace transfer_filesystem {

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

static const std::size_t MAX_REMOVE_DEPTH = 256;

void authorize_path_if_present(const TransferPathAuthorizer& authorizer, const std::string& path) {
    if (authorizer) {
        authorizer(path);
    }
}

void authorize_existing_path_recursive(
    const std::string& path,
    const TransferPathAuthorizer& authorizer,
    std::size_t depth
) {
    if (!authorizer || !path_exists(path)) {
        return;
    }
    if (depth > MAX_REMOVE_DEPTH) {
        throw std::runtime_error(
            "authorize_existing_path exceeded maximum depth of " + std::to_string(MAX_REMOVE_DEPTH)
        );
    }

    authorizer(path);
    if (is_directory(path)) {
        const std::vector<DirectoryEntry> entries = list_directory_entries(path);
        for (std::size_t i = 0; i < entries.size(); ++i) {
            authorize_existing_path_recursive(join_path(path, entries[i].name), authorizer, depth + 1);
        }
    }
}

void remove_existing_path_recursive(const std::string& path, std::size_t depth) {
    if (depth > MAX_REMOVE_DEPTH) {
        throw std::runtime_error("remove_existing_path exceeded maximum depth of " + std::to_string(MAX_REMOVE_DEPTH));
    }

    if (!path_exists(path)) {
        return;
    }

    if (is_directory(path)) {
        const std::vector<DirectoryEntry> entries = list_directory_entries(path);
        for (std::size_t i = 0; i < entries.size(); ++i) {
            remove_existing_path_recursive(join_path(path, entries[i].name), depth + 1);
        }
        if (!path_utils::remove_directory(path)) {
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
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata_no_follow(path, &metadata) && metadata.is_symlink;
}

bool path_exists(const std::string& path) {
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata_no_follow(path, &metadata);
}

bool is_regular_file(const std::string& path) {
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata_no_follow(path, &metadata) && metadata.is_regular_file;
}

bool is_regular_file_follow(const std::string& path) {
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata(path, &metadata) && metadata.is_regular_file;
}

bool is_directory(const std::string& path) {
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata_no_follow(path, &metadata) && metadata.is_directory;
}

bool is_directory_follow(const std::string& path) {
    path_utils::PathMetadata metadata;
    return path_utils::path_metadata(path, &metadata) && metadata.is_directory;
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
        if (!path_utils::remove_path(path)) {
            throw std::runtime_error("unable to remove existing file " + path);
        }
    }
    if (!path_utils::create_symlink(target, path)) {
        throw std::runtime_error("unable to create symlink " + path);
    }
#endif
}

std::vector<DirectoryEntry> list_directory_entries(const std::string& path) {
    std::vector<DirectoryEntry> entries;
    const std::vector<path_utils::DirectoryEntryInfo> platform_entries = path_utils::read_directory_entries(path);
    for (std::size_t i = 0; i < platform_entries.size(); ++i) {
        const path_utils::DirectoryEntryInfo& entry = platform_entries[i];
        entries.push_back(DirectoryEntry{entry.name, entry.is_directory, entry.is_regular_file, entry.is_symlink});
    }

    std::sort(entries.begin(), entries.end(), [](const DirectoryEntry& left, const DirectoryEntry& right) {
        return left.name < right.name;
    });
    return entries;
}

void replace_existing_path(const std::string& path, const TransferPathAuthorizer& authorizer) {
    if (!path_exists(path)) {
        return;
    }
    authorize_existing_path_recursive(path, authorizer, 0);
    remove_existing_path(path);
}

bool prepare_destination_path(
    const std::string& absolute_path,
    TransferSourceType source_type,
    TransferOverwrite overwrite,
    bool create_parent,
    const TransferPathAuthorizer& authorizer
) {
    authorize_path_if_present(authorizer, absolute_path);
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
                throw TransferFailure(
                    TransferRpcCode::DestinationUnsupported,
                    "destination path is not a regular file"
                );
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
        if (source_type == TransferSourceType::Multiple) {
            ensure_not_existing_symlink(absolute_path);
            if (!is_directory(absolute_path)) {
                throw TransferFailure(TransferRpcCode::DestinationUnsupported, "destination path is not a directory");
            }
            return true;
        }
        replace_existing_path(absolute_path, authorizer);
    }

    return existed && overwrite == TransferOverwrite::Replace;
}

} // namespace transfer_filesystem
