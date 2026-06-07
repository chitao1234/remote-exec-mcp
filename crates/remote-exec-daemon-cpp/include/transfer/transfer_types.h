#pragma once

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
