#pragma once

#include <string>
#include <vector>

#include "transfer/transfer_types.h"

namespace transfer_filesystem {

struct DirectoryEntry {
    std::string name;
    bool is_directory;
    bool is_regular_file;
    bool is_symlink;
};

bool is_absolute_path(const std::string& path);
bool is_symlink_path(const std::string& path);
bool path_exists(const std::string& path);
bool is_regular_file(const std::string& path);
bool is_regular_file_follow(const std::string& path);
bool is_directory(const std::string& path);
bool is_directory_follow(const std::string& path);
std::string join_path(const std::string& base, const std::string& child);
void make_directory_if_missing(const std::string& path);
void ensure_parent_directory(const std::string& path, bool create_parent);
void ensure_not_existing_symlink(const std::string& path);
void write_symlink(const std::string& target, const std::string& path);
std::vector<DirectoryEntry> list_directory_entries(const std::string& path);
void replace_existing_path(const std::string& path, const TransferPathAuthorizer& authorizer);
bool prepare_destination_path(const std::string& absolute_path,
                              TransferSourceType source_type,
                              TransferOverwrite overwrite,
                              bool create_parent,
                              const TransferPathAuthorizer& authorizer);

} // namespace transfer_filesystem
