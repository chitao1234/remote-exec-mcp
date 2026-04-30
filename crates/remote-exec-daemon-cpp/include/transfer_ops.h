#pragma once

#include <cstdint>
#include <string>
#include <vector>

struct TransferWarning {
    std::string code;
    std::string message;
};

struct ExportedPayload {
    std::string source_type;
    std::string bytes;
    std::vector<TransferWarning> warnings;
};

struct ImportSummary {
    std::string source_type;
    std::uint64_t bytes_copied;
    std::uint64_t files_copied;
    std::uint64_t directories_copied;
    bool replaced;
    std::vector<TransferWarning> warnings;
};

struct PathInfo {
    bool exists;
    bool is_directory;
};

ExportedPayload export_path(
    const std::string& absolute_path,
    const std::string& transfer_mode = "lenient",
    const std::string& symlink_mode = "preserve"
);
PathInfo path_info(const std::string& absolute_path);
ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    const std::string& transfer_mode = "lenient",
    const std::string& symlink_mode = "preserve"
);
