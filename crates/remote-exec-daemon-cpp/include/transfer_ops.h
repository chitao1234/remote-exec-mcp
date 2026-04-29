#pragma once

#include <cstdint>
#include <string>

struct ExportedPayload {
    std::string source_type;
    std::string bytes;
};

struct ImportSummary {
    std::string source_type;
    std::uint64_t bytes_copied;
    std::uint64_t files_copied;
    std::uint64_t directories_copied;
    bool replaced;
};

ExportedPayload export_path(const std::string& absolute_path);
ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
);
