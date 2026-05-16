#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <string>
#include <vector>

enum class TransferSourceType {
    File,
    Directory,
    Multiple,
};

enum class TransferSymlinkMode {
    Preserve,
    Follow,
    Skip,
};

enum class TransferOverwrite {
    Fail,
    Merge,
    Replace,
};

const char* transfer_source_type_wire_value(TransferSourceType source_type);
bool parse_transfer_source_type_wire_value(const std::string& value, TransferSourceType* source_type);
const char* transfer_symlink_mode_wire_value(TransferSymlinkMode symlink_mode);
bool parse_transfer_symlink_mode_wire_value(const std::string& value, TransferSymlinkMode* symlink_mode);
const char* transfer_overwrite_wire_value(TransferOverwrite overwrite);
bool parse_transfer_overwrite_wire_value(const std::string& value, TransferOverwrite* overwrite);

struct TransferWarning {
    std::string code;
    std::string message;
};

struct ExportedPayload {
    TransferSourceType source_type;
    std::string bytes;
};

struct ImportSummary {
    TransferSourceType source_type;
    std::uint64_t bytes_copied;
    std::uint64_t files_copied;
    std::uint64_t directories_copied;
    bool replaced;
    std::vector<TransferWarning> warnings;
};

struct TransferLimitConfig {
    std::uint64_t max_archive_bytes;
    std::uint64_t max_entry_bytes;
};

static constexpr std::uint64_t DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES = 512ULL * 1024ULL * 1024ULL;
static constexpr std::uint64_t DEFAULT_TRANSFER_MAX_ENTRY_BYTES = 512ULL * 1024ULL * 1024ULL;

inline TransferLimitConfig default_transfer_limit_config() {
    TransferLimitConfig config;
    config.max_archive_bytes = DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES;
    config.max_entry_bytes = DEFAULT_TRANSFER_MAX_ENTRY_BYTES;
    return config;
}

typedef std::function<void(const std::string&)> TransferPathAuthorizer;

struct PathInfo {
    bool exists;
    bool is_directory;
};

class TransferArchiveReader {
public:
    virtual ~TransferArchiveReader();
    virtual bool read_exact_or_eof(char* data, std::size_t size) = 0;
};

class TransferArchiveSink {
public:
    virtual ~TransferArchiveSink();
    virtual void write(const char* data, std::size_t size) = 0;

    void write_string(const std::string& data);
};

ExportedPayload export_path(const std::string& absolute_path,
                            TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                            const std::vector<std::string>& exclude = std::vector<std::string>());
TransferSourceType export_path_source_type(const std::string& absolute_path,
                                           TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve);
TransferSourceType export_path_to_sink(TransferArchiveSink& sink,
                                       const std::string& absolute_path,
                                       TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                                       const std::vector<std::string>& exclude = std::vector<std::string>());
void export_path_to_sink_as(TransferArchiveSink& sink,
                            const std::string& absolute_path,
                            TransferSourceType source_type,
                            TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                            const std::vector<std::string>& exclude = std::vector<std::string>());
PathInfo path_info(const std::string& absolute_path);
ImportSummary import_path(const std::string& bytes,
                          TransferSourceType source_type,
                          const std::string& absolute_path,
                          TransferOverwrite overwrite,
                          bool create_parent,
                          TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                          const TransferLimitConfig& limits = default_transfer_limit_config(),
                          const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
ImportSummary import_path_from_reader(TransferArchiveReader& reader,
                                      TransferSourceType source_type,
                                      const std::string& absolute_path,
                                      TransferOverwrite overwrite,
                                      bool create_parent,
                                      TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                                      const TransferLimitConfig& limits = default_transfer_limit_config(),
                                      const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
