#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <fstream>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#ifndef _WIN32
#include <sys/stat.h>
#endif

#include "json.hpp"
#include "rpc_failures.h"
#include "transfer_ops_internal.h"

using Json = nlohmann::json;

namespace transfer_ops_internal {

extern const std::size_t TAR_BLOCK_SIZE = 512;
extern const char SINGLE_FILE_ENTRY[] = ".remote-exec-file";
extern const char TRANSFER_SUMMARY_ENTRY[] = ".remote-exec-transfer-summary.json";

namespace {

void write_string_field(std::string* header, std::size_t offset, std::size_t width, const std::string& value) {
    const std::size_t length = std::min(width, value.size());
    if (length > 0) {
        header->replace(offset, length, value.substr(0, length));
    }
}

void write_octal_field(std::string* header, std::size_t offset, std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer,
        sizeof(buffer),
        "%0*llo",
        static_cast<int>(width - 1),
        static_cast<unsigned long long>(value)
    );
    const std::string digits(buffer);
    if (digits.size() > width - 1) {
        throw std::runtime_error("tar numeric field overflow");
    }

    std::string field(width, '\0');
    field.replace(width - 1 - digits.size(), digits.size(), digits);
    field[width - 1] = ' ';
    header->replace(offset, width, field);
}

void write_tar_checksum(std::string* header) {
    std::fill(header->begin() + 148, header->begin() + 156, ' ');
    unsigned int checksum = 0;
    for (std::size_t i = 0; i < header->size(); ++i) {
        checksum += static_cast<unsigned char>((*header)[i]);
    }
    write_octal_field(header, 148, 8, checksum);
}

std::string path_for_header_name(const std::string& path, bool long_name_emitted) {
    if (path.size() <= 100) {
        return path;
    }
    if (!long_name_emitted) {
        throw std::runtime_error("tar path too long without GNU long name header");
    }
    return path.substr(0, 100);
}

std::size_t tar_padding(std::uint64_t size) {
    const std::size_t remainder = static_cast<std::size_t>(size % TAR_BLOCK_SIZE);
    return remainder == 0 ? 0 : TAR_BLOCK_SIZE - remainder;
}

void append_padding(TransferArchiveSink* archive, std::uint64_t size) {
    const std::size_t padding = tar_padding(size);
    if (padding == 0U) {
        return;
    }
    const std::string zeros(padding, '\0');
    archive->write_string(zeros);
}

void append_padded_body(TransferArchiveSink* archive, const std::string& body) {
    archive->write_string(body);
    append_padding(archive, body.size());
}

void append_tar_header(
    TransferArchiveSink* archive,
    const std::string& path,
    char typeflag,
    std::uint64_t size,
    std::uint64_t mode,
    const std::string& link_name = std::string(),
    bool long_name_emitted = false
) {
    std::string header(TAR_BLOCK_SIZE, '\0');
    write_string_field(&header, 0, 100, path_for_header_name(path, long_name_emitted));
    write_octal_field(&header, 100, 8, mode);
    write_octal_field(&header, 108, 8, 0);
    write_octal_field(&header, 116, 8, 0);
    write_octal_field(&header, 124, 12, size);
    write_octal_field(&header, 136, 12, 0);
    header[156] = typeflag;
    if (!link_name.empty()) {
        write_string_field(&header, 157, 100, link_name);
    }
    write_string_field(&header, 257, 6, "ustar ");
    header[263] = ' ';
    header[264] = '\0';
    write_tar_checksum(&header);
    archive->write_string(header);
}

std::string field_string(const char* data, std::size_t size) {
    std::size_t length = 0;
    while (length < size && data[length] != '\0') {
        ++length;
    }
    return std::string(data, length);
}

std::uint64_t parse_octal_field(const char* data, std::size_t size) {
    std::size_t index = 0;
    while (index < size && (data[index] == ' ' || data[index] == '\0')) {
        ++index;
    }
    std::uint64_t value = 0;
    while (index < size && data[index] >= '0' && data[index] <= '7') {
        value = (value * 8) + static_cast<std::uint64_t>(data[index] - '0');
        ++index;
    }
    return value;
}

std::uint64_t checked_add_u64(
    std::uint64_t left,
    std::uint64_t right,
    const std::string& label
) {
    if (left > std::numeric_limits<std::uint64_t>::max() - right) {
        throw TransferFailure(
            TransferRpcCode::TransferFailed,
            label + " is too large"
        );
    }
    return left + right;
}

bool checksum_valid(const char* block) {
    const std::uint64_t stored = parse_octal_field(block + 148, 8);
    std::uint64_t computed = 0;
    for (std::size_t i = 0; i < TAR_BLOCK_SIZE; ++i) {
        if (i >= 148 && i < 156) {
            computed += static_cast<unsigned char>(' ');
        } else {
            computed += static_cast<unsigned char>(block[i]);
        }
    }
    return stored == computed;
}

Json transfer_warnings_json(const std::vector<TransferWarning>& warnings) {
    Json json = Json::array();
    for (std::size_t i = 0; i < warnings.size(); ++i) {
        json.push_back(Json{
            {"code", warnings[i].code},
            {"message", warnings[i].message},
        });
    }
    return json;
}

std::string header_path(const char* block) {
    const std::string name = field_string(block, 100);
    const std::string prefix = field_string(block + 345, 155);
    if (prefix.empty()) {
        return name;
    }
    if (name.empty()) {
        return prefix;
    }
    return prefix + "/" + name;
}

}  // namespace

void append_archive_terminator(TransferArchiveSink* archive) {
    const std::string terminator(TAR_BLOCK_SIZE * 2, '\0');
    archive->write_string(terminator);
}

void append_gnu_long_name(TransferArchiveSink* archive, const std::string& path) {
    const std::string body = path + '\0';
    append_tar_header(archive, "././@LongLink", 'L', body.size(), 0644);
    append_padded_body(archive, body);
}

void append_directory_entry(TransferArchiveSink* archive, const std::string& rel_path) {
    const bool long_name_emitted = rel_path.size() > 100;
    if (long_name_emitted) {
        append_gnu_long_name(archive, rel_path);
    }
    append_tar_header(archive, rel_path, '5', 0, 0755, std::string(), long_name_emitted);
}

void append_file_entry(TransferArchiveSink* archive, const std::string& rel_path, const std::string& body) {
    const bool long_name_emitted = rel_path.size() > 100;
    if (long_name_emitted) {
        append_gnu_long_name(archive, rel_path);
    }
    append_tar_header(
        archive,
        rel_path,
        '0',
        body.size(),
        0644,
        std::string(),
        long_name_emitted
    );
    append_padded_body(archive, body);
}

void append_file_entry_from_path(
    TransferArchiveSink* archive,
    const std::string& rel_path,
    const std::string& source_path
) {
#ifndef _WIN32
    struct stat st;
    if (stat(source_path.c_str(), &st) != 0) {
        throw TransferFailure(
            TransferRpcCode::SourceMissing,
            "transfer source missing"
        );
    }
    const std::uint64_t mode = static_cast<std::uint64_t>(st.st_mode & 0777);
#else
    const std::uint64_t mode = 0644;
#endif
    std::ifstream input(source_path.c_str(), std::ios::binary | std::ios::ate);
    if (!input) {
        throw TransferFailure(
            TransferRpcCode::SourceMissing,
            "transfer source missing"
        );
    }
    const std::ifstream::pos_type end_position = input.tellg();
    if (end_position < 0) {
        throw std::runtime_error("unable to read transfer source size");
    }
    const std::uint64_t file_size = static_cast<std::uint64_t>(end_position);
    input.seekg(0, std::ios::beg);
    if (!input) {
        throw std::runtime_error("unable to read transfer source");
    }

    const bool long_name_emitted = rel_path.size() > 100;
    if (long_name_emitted) {
        append_gnu_long_name(archive, rel_path);
    }
    append_tar_header(archive, rel_path, '0', file_size, mode, std::string(), long_name_emitted);

    char buffer[8192];
    std::uint64_t remaining = file_size;
    while (remaining > 0U) {
        const std::size_t requested =
            remaining < sizeof(buffer) ? static_cast<std::size_t>(remaining) : sizeof(buffer);
        input.read(buffer, static_cast<std::streamsize>(requested));
        const std::streamsize received = input.gcount();
        if (received <= 0 || static_cast<std::size_t>(received) != requested) {
            throw std::runtime_error("unable to read transfer source");
        }
        archive->write(buffer, static_cast<std::size_t>(received));
        remaining -= static_cast<std::uint64_t>(received);
    }
    append_padding(archive, file_size);
}

#ifndef _WIN32
void append_symlink_entry(TransferArchiveSink* archive, const std::string& rel_path, const std::string& target) {
    const bool long_name_emitted = rel_path.size() > 100;
    if (long_name_emitted) {
        append_gnu_long_name(archive, rel_path);
    }
    if (target.size() > 100) {
        throw std::runtime_error("tar symlink target too long");
    }
    append_tar_header(archive, rel_path, '2', 0, 0777, target, long_name_emitted);
}
#endif

bool is_zero_block(const char* block) {
    for (std::size_t i = 0; i < TAR_BLOCK_SIZE; ++i) {
        if (block[i] != '\0') {
            return false;
        }
    }
    return true;
}

bool is_transfer_summary_path(const std::string& path) {
    return path == TRANSFER_SUMMARY_ENTRY;
}

void append_transfer_summary_entry(
    TransferArchiveSink* archive,
    const std::vector<TransferWarning>& warnings
) {
    if (warnings.empty()) {
        return;
    }
    const Json summary = Json{{"warnings", transfer_warnings_json(warnings)}};
    append_file_entry(archive, TRANSFER_SUMMARY_ENTRY, summary.dump());
}

std::vector<TransferWarning> read_transfer_summary(const std::string& body) {
    std::vector<TransferWarning> warnings;
    const Json summary = Json::parse(body);
    const Json raw_warnings = summary.value("warnings", Json::array());
    for (std::size_t i = 0; i < raw_warnings.size(); ++i) {
        warnings.push_back(TransferWarning{
            raw_warnings[i].value("code", std::string()),
            raw_warnings[i].value("message", std::string()),
        });
    }
    return warnings;
}

void append_warnings(
    std::vector<TransferWarning>* destination,
    const std::vector<TransferWarning>& source
) {
    destination->insert(destination->end(), source.begin(), source.end());
}

TarHeaderView parse_header(const char* block) {
    if (!checksum_valid(block)) {
        throw TransferFailure(
            TransferRpcCode::TransferFailed,
            "invalid tar header checksum"
        );
    }
    const char raw_type = block[156];
    return TarHeaderView{
        header_path(block),
        raw_type == '\0' ? '0' : raw_type,
        parse_octal_field(block + 124, 12),
        parse_octal_field(block + 100, 8),
        field_string(block + 157, 100),
    };
}

void ensure_u64_fits_size_t(std::uint64_t value, const std::string& label) {
    if (value > static_cast<std::uint64_t>(std::numeric_limits<std::size_t>::max())) {
        throw TransferFailure(
            TransferRpcCode::TransferFailed,
            label + " is too large for this platform"
        );
    }
}

void ensure_transfer_entry_within_limits(
    std::uint64_t entry_size,
    std::uint64_t copied_so_far,
    const TransferLimitConfig& limits
) {
    if (entry_size > limits.max_entry_bytes) {
        std::ostringstream message;
        message << "archive entry size " << entry_size
                << " exceeds transfer entry limit " << limits.max_entry_bytes;
        throw TransferFailure(TransferRpcCode::TransferFailed, message.str());
    }
    if (copied_so_far > limits.max_archive_bytes ||
        entry_size > limits.max_archive_bytes - copied_so_far) {
        std::ostringstream message;
        message << "archive byte count exceeds transfer archive limit "
                << limits.max_archive_bytes;
        throw TransferFailure(TransferRpcCode::TransferFailed, message.str());
    }
}

std::size_t padded_length(std::uint64_t size) {
    const std::uint64_t padded = checked_add_u64(
        size,
        static_cast<std::uint64_t>(tar_padding(size)),
        "tar entry size"
    );
    ensure_u64_fits_size_t(padded, "tar entry size");
    return static_cast<std::size_t>(padded);
}

std::string read_gnu_long_name(
    const std::string& archive,
    std::size_t body_offset,
    std::uint64_t size
) {
    if (body_offset + padded_length(size) > archive.size()) {
        throw TransferFailure(
            TransferRpcCode::TransferFailed,
            "truncated tar entry body"
        );
    }
    ensure_u64_fits_size_t(size, "GNU long name entry size");
    std::string value = archive.substr(body_offset, static_cast<std::size_t>(size));
    while (!value.empty() && value[value.size() - 1] == '\0') {
        value.erase(value.size() - 1);
    }
    return value;
}

void validate_transfer_options(const ExportOptions& options) {
    (void)options;
}

}  // namespace transfer_ops_internal
