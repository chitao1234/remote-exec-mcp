#pragma once

#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

namespace path_utils {

struct PathMetadata {
    PathMetadata()
        : exists(false), is_regular_file(false), is_directory(false), is_symlink(false), size(0), mode_bits(0),
          has_mode_bits(false) {}

    bool exists;
    bool is_regular_file;
    bool is_directory;
    bool is_symlink;
    std::uint64_t size;
    unsigned int mode_bits;
    bool has_mode_bits;
};

struct FileIdentity {
    FileIdentity() : valid(false), device(0ULL), file(0ULL) {}

    bool valid;
    unsigned long long device;
    unsigned long long file;
};

struct DirectoryEntryInfo {
    std::string name;
    bool is_directory;
    bool is_regular_file;
    bool is_symlink;
};

char native_separator();
std::string parent_directory(const std::string& path);
std::string join_path(const std::string& base, const std::string& child);
void make_directory_if_missing(const std::string& path);
void create_parent_directories(const std::string& path);
FILE* open_file(const std::string& path, const char* mode);
bool path_metadata(const std::string& path, PathMetadata* metadata);
bool path_metadata_no_follow(const std::string& path, PathMetadata* metadata);
bool file_identity(const std::string& path, FileIdentity* identity);
bool remove_path(const std::string& path);
bool remove_directory(const std::string& path);
bool rename_path(const std::string& source, const std::string& destination);
bool set_path_mode(const std::string& path, unsigned int mode);
bool add_executable_bits(FILE* file);
bool read_symlink_target(const std::string& path, std::string* target);
bool create_symlink(const std::string& target, const std::string& path);
std::vector<DirectoryEntryInfo> read_directory_entries(const std::string& path);

#ifdef _WIN32
std::wstring wide_from_utf8(const std::string& value);
std::string utf8_from_wide(const std::wstring& value);
#endif

} // namespace path_utils
