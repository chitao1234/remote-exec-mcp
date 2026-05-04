#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

#include "transfer_ops.h"

namespace transfer_ops_internal {

extern const std::size_t TAR_BLOCK_SIZE;
extern const char SINGLE_FILE_ENTRY[];
extern const char TRANSFER_SUMMARY_ENTRY[];

struct DirectoryEntry {
    std::string name;
    bool is_directory;
    bool is_regular_file;
    bool is_symlink;
};

struct TarHeaderView {
    std::string path;
    char typeflag;
    std::uint64_t size;
    std::uint64_t mode;
    std::string link_name;
};

struct ExportOptions {
    std::string symlink_mode;
    std::vector<std::string> exclude;
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
bool prepare_destination_path(
    const std::string& absolute_path,
    const std::string& source_type,
    const std::string& overwrite_mode,
    bool create_parent
);

void append_archive_terminator(TransferArchiveSink* archive);
void append_directory_entry(TransferArchiveSink* archive, const std::string& rel_path);
void append_file_entry(TransferArchiveSink* archive, const std::string& rel_path, const std::string& body);
void append_file_entry_from_path(
    TransferArchiveSink* archive,
    const std::string& rel_path,
    const std::string& source_path
);
#ifndef _WIN32
void append_symlink_entry(
    TransferArchiveSink* archive,
    const std::string& rel_path,
    const std::string& target
);
#endif
bool is_zero_block(const char* block);
TarHeaderView parse_header(const char* block);
std::size_t padded_length(std::uint64_t size);
std::string read_gnu_long_name(
    const std::string& archive,
    std::size_t body_offset,
    std::uint64_t size
);
bool is_transfer_summary_path(const std::string& path);
void append_transfer_summary_entry(
    TransferArchiveSink* archive,
    const std::vector<TransferWarning>& warnings
);
std::vector<TransferWarning> read_transfer_summary(const std::string& body);
void append_warnings(
    std::vector<TransferWarning>* destination,
    const std::vector<TransferWarning>& source
);

void validate_transfer_options(const ExportOptions& options);

}  // namespace transfer_ops_internal
