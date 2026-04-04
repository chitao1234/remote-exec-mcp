#pragma once

#include <cstdint>
#include <string>

struct ExportedFile {
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

ExportedFile export_file(const std::string& absolute_path);
ImportSummary import_file(
    const std::string& bytes,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
);
