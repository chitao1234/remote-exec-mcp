#pragma once

#include <cstddef>
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

ExportedPayload export_path(
    const std::string& absolute_path,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
std::string export_path_source_type(
    const std::string& absolute_path,
    const std::string& symlink_mode = "preserve"
);
std::string export_path_to_sink(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
void export_path_to_sink_as(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& source_type,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
PathInfo path_info(const std::string& absolute_path);
ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    const std::string& symlink_mode = "preserve"
);
ImportSummary import_path_from_reader(
    TransferArchiveReader& reader,
    const std::string& source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    const std::string& symlink_mode = "preserve"
);
