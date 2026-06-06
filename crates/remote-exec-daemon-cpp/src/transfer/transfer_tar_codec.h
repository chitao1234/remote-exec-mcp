#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

#include "transfer/transfer_ops.h"

namespace transfer_tar_codec {

extern const std::size_t TAR_BLOCK_SIZE;
extern const char SINGLE_FILE_ENTRY[];
extern const char TRANSFER_SUMMARY_ENTRY[];

struct TarHeaderView {
    std::string path;
    char typeflag;
    std::uint64_t size;
    std::uint64_t mode;
    std::string link_name;
};

void append_archive_terminator(TransferArchiveSink* archive);
void append_directory_entry(TransferArchiveSink* archive, const std::string& rel_path);
void append_file_entry(TransferArchiveSink* archive, const std::string& rel_path, const std::string& body);
void append_file_entry_from_path(TransferArchiveSink* archive,
                                 const std::string& rel_path,
                                 const std::string& source_path);
#ifndef _WIN32
void append_symlink_entry(TransferArchiveSink* archive, const std::string& rel_path, const std::string& target);
#endif
bool is_zero_block(const char* block);
bool is_transfer_summary_path(const std::string& path);
void append_transfer_summary_entry(TransferArchiveSink* archive, const std::vector<TransferWarning>& warnings);
TarHeaderView parse_header(const char* block);
void ensure_u64_fits_size_t(std::uint64_t value, const std::string& label);
void ensure_transfer_entry_within_limits(std::uint64_t entry_size,
                                         std::uint64_t copied_so_far,
                                         const TransferLimitConfig& limits);
std::uint64_t entry_padding(std::uint64_t size);
std::uint64_t entry_body_with_padding(std::uint64_t size);
void require_archive_terminator(TransferArchiveReader& reader);
void skip_entry_padding(TransferArchiveReader& reader, std::uint64_t size);
std::string read_limited_metadata_string(TransferArchiveReader& reader,
                                         std::uint64_t size,
                                         const TransferLimitConfig& limits,
                                         const std::string& error_message);
std::string
read_gnu_long_name_from_reader(TransferArchiveReader& reader, std::uint64_t size, const TransferLimitConfig& limits);

} // namespace transfer_tar_codec
