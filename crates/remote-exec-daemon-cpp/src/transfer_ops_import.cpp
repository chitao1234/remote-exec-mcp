#include <algorithm>
#include <cctype>
#include <stdexcept>
#include <string>
#include <vector>

#include "transfer_ops_internal.h"

namespace {

using namespace transfer_ops_internal;

std::string trim_trailing_slashes(std::string value) {
    while (value.size() > 1 && !value.empty() && value[value.size() - 1] == '/') {
        value.erase(value.size() - 1);
    }
    return value;
}

std::vector<std::string> split_archive_path(const std::string& path) {
    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = 0; i < path.size(); ++i) {
        const char ch = path[i];
        if (ch == '/') {
            parts.push_back(current);
            current.clear();
            continue;
        }
        current.push_back(ch);
    }
    parts.push_back(current);
    return parts;
}

std::string normalize_archive_separators(std::string value) {
    std::replace(value.begin(), value.end(), '\\', '/');
    return value;
}

std::string validate_relative_archive_path(const std::string& raw_path) {
    std::string normalized = normalize_archive_separators(raw_path);
    while (normalized.rfind("./", 0) == 0) {
        normalized.erase(0, 2);
    }
    normalized = trim_trailing_slashes(normalized);

    if (normalized.empty() || normalized == ".") {
        return ".";
    }
    if (normalized[0] == '/') {
        throw std::runtime_error("archive path must be relative");
    }
    if (normalized.size() >= 2 &&
        std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
        normalized[1] == ':') {
        throw std::runtime_error("archive path must be relative");
    }
    if (normalized.rfind("//", 0) == 0) {
        throw std::runtime_error("archive path must be relative");
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    std::vector<std::string> cleaned;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        const std::string& part = parts[i];
        if (part.empty()) {
            throw std::runtime_error("archive path contains empty component");
        }
        if (part == "." || part == "..") {
            throw std::runtime_error("archive path escapes destination");
        }
        cleaned.push_back(part);
    }

    std::string result;
    for (std::size_t i = 0; i < cleaned.size(); ++i) {
        if (i != 0) {
            result.push_back('/');
        }
        result += cleaned[i];
    }
    return result;
}

std::string materialize_archive_path(
    const std::string& destination_root,
    const std::string& relative_archive_path
) {
    if (relative_archive_path == ".") {
        return destination_root;
    }

    const std::vector<std::string> parts = split_archive_path(relative_archive_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = join_path(path, parts[i]);
    }
    return path;
}

void ensure_no_existing_symlink_in_path(
    const std::string& destination_root,
    const std::string& relative_archive_path
) {
    ensure_not_existing_symlink(destination_root);
    if (relative_archive_path == ".") {
        return;
    }

    const std::vector<std::string> parts = split_archive_path(relative_archive_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = join_path(path, parts[i]);
        ensure_not_existing_symlink(path);
    }
}

void consume_file_archive_tail(
    const std::string& archive,
    std::size_t offset,
    std::vector<TransferWarning>* warnings
) {
    std::string pending_long_name;
    while (offset < archive.size()) {
        if (archive.size() - offset < TAR_BLOCK_SIZE) {
            throw std::runtime_error("truncated tar header");
        }
        const char* block = archive.data() + offset;
        if (is_zero_block(block)) {
            offset += TAR_BLOCK_SIZE;
            continue;
        }

        const TarHeaderView header = parse_header(block);
        offset += TAR_BLOCK_SIZE;
        if (offset + padded_length(header.size) > archive.size()) {
            throw std::runtime_error("truncated tar entry body");
        }

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name(archive, offset, header.size);
            offset += padded_length(header.size);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        if (!is_transfer_summary_path(raw_path) || header.typeflag != '0') {
            throw std::runtime_error("file archive contains extra entries");
        }
        append_warnings(
            warnings,
            read_transfer_summary(archive.substr(offset, static_cast<std::size_t>(header.size)))
        );
        offset += padded_length(header.size);
    }

    if (!pending_long_name.empty()) {
        throw std::runtime_error("dangling GNU long name entry");
    }
}

ImportSummary import_file_from_tar(
    const std::string& archive,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    const std::string& symlink_mode
) {
    const bool replaced = prepare_destination_path(absolute_path, "file", overwrite_mode, create_parent);
    ensure_not_existing_symlink(absolute_path);

    if (archive.size() < TAR_BLOCK_SIZE) {
        throw std::runtime_error("archive is empty");
    }

    const TarHeaderView header = parse_header(archive.data());
    if (header.typeflag != '0' && header.typeflag != '2') {
        throw std::runtime_error("archive entry is not a regular file");
    }
    if (header.path != SINGLE_FILE_ENTRY) {
        throw std::runtime_error("file archive entry path must be " + std::string(SINGLE_FILE_ENTRY));
    }

    const std::size_t body_offset = TAR_BLOCK_SIZE;
    if (body_offset + padded_length(header.size) > archive.size()) {
        throw std::runtime_error("truncated tar entry body");
    }

    std::uint64_t bytes_copied = 0;
    if (header.typeflag == '2') {
        if (symlink_mode != "preserve") {
            throw std::runtime_error("archive contains unsupported symlink " + absolute_path);
        }
        write_symlink(header.link_name, absolute_path);
    } else {
        const std::string bytes = archive.substr(body_offset, static_cast<std::size_t>(header.size));
        write_binary_file(absolute_path, bytes);
        bytes_copied = static_cast<std::uint64_t>(bytes.size());
    }

    std::vector<TransferWarning> warnings;
    consume_file_archive_tail(archive, body_offset + padded_length(header.size), &warnings);

    return ImportSummary{
        "file",
        bytes_copied,
        1,
        0,
        replaced,
        warnings,
    };
}

ImportSummary import_directory_from_tar(
    const std::string& archive,
    const std::string& source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    const std::string& symlink_mode
) {
    const bool replaced = prepare_destination_path(absolute_path, source_type, overwrite_mode, create_parent);
    make_directory_if_missing(absolute_path);

    ImportSummary summary = {source_type, 0, 0, 1, replaced, std::vector<TransferWarning>()};
    std::size_t offset = 0;
    std::string pending_long_name;

    while (offset < archive.size()) {
        if (archive.size() - offset < TAR_BLOCK_SIZE) {
            throw std::runtime_error("truncated tar header");
        }

        const char* block = archive.data() + offset;
        if (is_zero_block(block)) {
            break;
        }

        const TarHeaderView header = parse_header(block);
        offset += TAR_BLOCK_SIZE;

        if (offset + padded_length(header.size) > archive.size()) {
            throw std::runtime_error("truncated tar entry body");
        }

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name(archive, offset, header.size);
            offset += padded_length(header.size);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        const std::string relative_path = validate_relative_archive_path(raw_path);
        if (is_transfer_summary_path(relative_path)) {
            if (header.typeflag != '0') {
                throw std::runtime_error("transfer summary archive entry is not a regular file");
            }
            append_warnings(
                &summary.warnings,
                read_transfer_summary(
                    archive.substr(offset, static_cast<std::size_t>(header.size))
                )
            );
            offset += padded_length(header.size);
            continue;
        }
        const std::string output_path = materialize_archive_path(absolute_path, relative_path);
        ensure_no_existing_symlink_in_path(absolute_path, relative_path);

        if (header.typeflag == '5') {
            if (relative_path != ".") {
                ensure_parent_directory(output_path, true);
                make_directory_if_missing(output_path);
                summary.directories_copied += 1;
            }
            offset += padded_length(header.size);
            continue;
        }

        if (header.typeflag == '2') {
            if (symlink_mode != "preserve") {
                throw std::runtime_error("archive contains unsupported symlink");
            }
            if (relative_path == ".") {
                throw std::runtime_error("archive symlink entry cannot target root");
            }
            write_symlink(header.link_name, output_path);
            summary.files_copied += 1;
            offset += padded_length(header.size);
            continue;
        }

        if (header.typeflag != '0') {
            throw std::runtime_error("archive contains unsupported entry");
        }
        if (relative_path == ".") {
            throw std::runtime_error("archive file entry cannot target root");
        }

        ensure_parent_directory(output_path, true);
        write_binary_file(
            output_path,
            archive.substr(offset, static_cast<std::size_t>(header.size))
        );
        summary.bytes_copied += header.size;
        summary.files_copied += 1;
        offset += padded_length(header.size);
    }

    if (!pending_long_name.empty()) {
        throw std::runtime_error("dangling GNU long name entry");
    }

    return summary;
}

}  // namespace

ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    const std::string& symlink_mode
) {
    ExportOptions options{symlink_mode.empty() ? "preserve" : symlink_mode};
    validate_transfer_options(options);
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }

    if (source_type == "file") {
        return import_file_from_tar(
            bytes,
            absolute_path,
            overwrite_mode,
            create_parent,
            options.symlink_mode
        );
    }
    if (source_type == "directory") {
        return import_directory_from_tar(
            bytes,
            source_type,
            absolute_path,
            overwrite_mode,
            create_parent,
            options.symlink_mode
        );
    }
    if (source_type == "multiple") {
        return import_directory_from_tar(
            bytes,
            source_type,
            absolute_path,
            overwrite_mode,
            create_parent,
            options.symlink_mode
        );
    }
    throw std::runtime_error("unsupported transfer source type");
}
