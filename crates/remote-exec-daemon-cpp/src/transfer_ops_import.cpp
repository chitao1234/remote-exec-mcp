#include <algorithm>
#include <cctype>
#include <fstream>
#include <stdexcept>
#include <string>
#include <vector>

#ifndef _WIN32
#include <sys/stat.h>
#endif

#include "rpc_failures.h"
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
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "archive path must be relative"
        );
    }
    if (normalized.size() >= 2 &&
        std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
        normalized[1] == ':') {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "archive path must be relative"
        );
    }
    if (normalized.rfind("//", 0) == 0) {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "archive path must be relative"
        );
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    std::vector<std::string> cleaned;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        const std::string& part = parts[i];
        if (part.empty()) {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive path contains empty component"
            );
        }
        if (part == "." || part == "..") {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive path escapes destination"
            );
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

class StringTransferArchiveReader : public TransferArchiveReader {
public:
    explicit StringTransferArchiveReader(const std::string* archive)
        : archive_(archive), offset_(0) {}

    bool read_exact_or_eof(char* data, std::size_t size) {
        if (size == 0U) {
            return true;
        }
        if (offset_ >= archive_->size()) {
            return false;
        }
        if (archive_->size() - offset_ < size) {
            throw TransferFailure(
                TransferRpcCode::TransferFailed,
                "truncated transfer body"
            );
        }
        std::copy(archive_->data() + offset_, archive_->data() + offset_ + size, data);
        offset_ += size;
        return true;
    }

private:
    const std::string* archive_;
    std::size_t offset_;
};

void read_exact_or_throw(
    TransferArchiveReader& reader,
    char* data,
    std::size_t size,
    const std::string& error_message
) {
    if (!reader.read_exact_or_eof(data, size)) {
        throw TransferFailure(TransferRpcCode::TransferFailed, error_message);
    }
}

std::string read_exact_string(
    TransferArchiveReader& reader,
    std::uint64_t size,
    const std::string& error_message
) {
    std::string body(static_cast<std::size_t>(size), '\0');
    if (!body.empty()) {
        read_exact_or_throw(reader, &body[0], body.size(), error_message);
    }
    return body;
}

void skip_exact(
    TransferArchiveReader& reader,
    std::uint64_t size,
    const std::string& error_message
) {
    char buffer[8192];
    std::uint64_t remaining = size;
    while (remaining > 0U) {
        const std::size_t requested =
            remaining < sizeof(buffer) ? static_cast<std::size_t>(remaining) : sizeof(buffer);
        read_exact_or_throw(reader, buffer, requested, error_message);
        remaining -= static_cast<std::uint64_t>(requested);
    }
}

std::uint64_t entry_padding(std::uint64_t size) {
    const std::uint64_t remainder = size % TAR_BLOCK_SIZE;
    return remainder == 0U ? 0U : static_cast<std::uint64_t>(TAR_BLOCK_SIZE) - remainder;
}

std::uint64_t entry_body_with_padding(std::uint64_t size) {
    return size + entry_padding(size);
}

void skip_entry_padding(TransferArchiveReader& reader, std::uint64_t size) {
    skip_exact(
        reader,
        entry_padding(size),
        "truncated tar entry body"
    );
}

std::string read_gnu_long_name_from_reader(TransferArchiveReader& reader, std::uint64_t size) {
    std::string value = read_exact_string(reader, size, "truncated tar entry body");
    skip_entry_padding(reader, size);
    while (!value.empty() && value[value.size() - 1] == '\0') {
        value.erase(value.size() - 1);
    }
    return value;
}

void copy_reader_to_file(
    TransferArchiveReader& reader,
    const std::string& path,
    std::uint64_t size,
    std::uint64_t mode
) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    if (!output) {
        throw std::runtime_error("unable to write destination file");
    }

    char buffer[8192];
    std::uint64_t remaining = size;
    while (remaining > 0U) {
        const std::size_t requested =
            remaining < sizeof(buffer) ? static_cast<std::size_t>(remaining) : sizeof(buffer);
        read_exact_or_throw(reader, buffer, requested, "truncated tar entry body");
        output.write(buffer, static_cast<std::streamsize>(requested));
        if (!output) {
            throw std::runtime_error("unable to write destination file");
        }
        remaining -= static_cast<std::uint64_t>(requested);
    }
    skip_entry_padding(reader, size);
    output.close();
    if (!output) {
        throw std::runtime_error("unable to write destination file");
    }
#ifndef _WIN32
    if ((mode & 0111U) != 0U) {
        struct stat st;
        if (stat(path.c_str(), &st) != 0) {
            throw std::runtime_error("unable to read destination file mode");
        }
        if (chmod(path.c_str(), st.st_mode | 0111) != 0) {
            throw std::runtime_error("unable to update destination file mode");
        }
    }
#else
    (void)mode;
#endif
}

TransferWarning skipped_symlink_warning(const std::string& path) {
    return TransferWarning{
        "transfer_skipped_symlink",
        "Skipped symlink transfer source entry `" + path + "`."
    };
}

enum SymlinkImportAction {
    SYMLINK_IMPORT_PRESERVE,
    SYMLINK_IMPORT_SKIP,
};

SymlinkImportAction symlink_import_action(
    TransferSymlinkMode symlink_mode,
    const std::string& error_path
) {
#ifdef _WIN32
    (void)symlink_mode;
    (void)error_path;
    return SYMLINK_IMPORT_SKIP;
#else
    if (symlink_mode == TransferSymlinkMode::Skip) {
        return SYMLINK_IMPORT_SKIP;
    }
    if (symlink_mode == TransferSymlinkMode::Preserve) {
        return SYMLINK_IMPORT_PRESERVE;
    }

    std::string message = "archive contains unsupported symlink";
    if (!error_path.empty()) {
        message += " " + error_path;
    }
    throw TransferFailure(TransferRpcCode::SourceUnsupported, message);
#endif
}

void consume_file_archive_tail(
    TransferArchiveReader& reader,
    std::vector<TransferWarning>* warnings
) {
    std::string pending_long_name;
    char block[TAR_BLOCK_SIZE];
    while (reader.read_exact_or_eof(block, TAR_BLOCK_SIZE)) {
        if (is_zero_block(block)) {
            continue;
        }

        const TarHeaderView header = parse_header(block);

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name_from_reader(reader, header.size);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        if (!is_transfer_summary_path(raw_path) || header.typeflag != '0') {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "file archive contains extra entries"
            );
        }
        append_warnings(
            warnings,
            read_transfer_summary(read_exact_string(reader, header.size, "truncated tar entry body"))
        );
        skip_entry_padding(reader, header.size);
    }

    if (!pending_long_name.empty()) {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "dangling GNU long name entry"
        );
    }
}

ImportSummary import_file_from_tar(
    TransferArchiveReader& reader,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    TransferSymlinkMode symlink_mode
) {
    const bool replaced = prepare_destination_path(
        absolute_path,
        TransferSourceType::File,
        overwrite_mode,
        create_parent
    );
    ensure_not_existing_symlink(absolute_path);

    char block[TAR_BLOCK_SIZE];
    read_exact_or_throw(reader, block, TAR_BLOCK_SIZE, "archive is empty");
    const TarHeaderView header = parse_header(block);
    if (header.typeflag != '0' && header.typeflag != '2') {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "archive entry is not a regular file"
        );
    }
    if (header.path != SINGLE_FILE_ENTRY) {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "file archive entry path must be " + std::string(SINGLE_FILE_ENTRY)
        );
    }

    std::uint64_t bytes_copied = 0;
    std::uint64_t files_copied = 1;
    std::vector<TransferWarning> warnings;
    if (header.typeflag == '2') {
        switch (symlink_import_action(symlink_mode, absolute_path)) {
        case SYMLINK_IMPORT_SKIP:
            warnings.push_back(skipped_symlink_warning(absolute_path));
            files_copied = 0;
            break;
        case SYMLINK_IMPORT_PRESERVE:
            write_symlink(header.link_name, absolute_path);
            break;
        }
        skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
    } else {
        copy_reader_to_file(reader, absolute_path, header.size, header.mode);
        bytes_copied = header.size;
    }

    consume_file_archive_tail(reader, &warnings);

    return ImportSummary{
        TransferSourceType::File,
        bytes_copied,
        files_copied,
        0,
        replaced,
        warnings,
    };
}

ImportSummary import_directory_from_tar(
    TransferArchiveReader& reader,
    TransferSourceType source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    TransferSymlinkMode symlink_mode
) {
    const bool replaced = prepare_destination_path(absolute_path, source_type, overwrite_mode, create_parent);
    make_directory_if_missing(absolute_path);

    ImportSummary summary = {source_type, 0, 0, 1, replaced, std::vector<TransferWarning>()};
    std::string pending_long_name;
    char block[TAR_BLOCK_SIZE];

    while (reader.read_exact_or_eof(block, TAR_BLOCK_SIZE)) {
        if (is_zero_block(block)) {
            break;
        }

        const TarHeaderView header = parse_header(block);

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name_from_reader(reader, header.size);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        const std::string relative_path = validate_relative_archive_path(raw_path);
        if (is_transfer_summary_path(relative_path)) {
            if (header.typeflag != '0') {
                throw TransferFailure(
                    TransferRpcCode::SourceUnsupported,
                    "transfer summary archive entry is not a regular file"
                );
            }
            append_warnings(
                &summary.warnings,
                read_transfer_summary(
                    read_exact_string(reader, header.size, "truncated tar entry body")
                )
            );
            skip_entry_padding(reader, header.size);
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
            skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
            continue;
        }

        if (header.typeflag == '2') {
            switch (symlink_import_action(symlink_mode, "")) {
            case SYMLINK_IMPORT_SKIP:
                summary.warnings.push_back(skipped_symlink_warning(output_path));
                skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
                continue;
            case SYMLINK_IMPORT_PRESERVE:
                break;
            }
            if (relative_path == ".") {
                throw TransferFailure(
                    TransferRpcCode::SourceUnsupported,
                    "archive symlink entry cannot target root"
                );
            }
            write_symlink(header.link_name, output_path);
            summary.files_copied += 1;
            skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
            continue;
        }

        if (header.typeflag != '0') {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive contains unsupported entry"
            );
        }
        if (relative_path == ".") {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive file entry cannot target root"
            );
        }

        ensure_parent_directory(output_path, true);
        copy_reader_to_file(reader, output_path, header.size, header.mode);
        summary.bytes_copied += header.size;
        summary.files_copied += 1;
    }

    if (!pending_long_name.empty()) {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "dangling GNU long name entry"
        );
    }

    return summary;
}

}  // namespace

ImportSummary import_path(
    const std::string& bytes,
    TransferSourceType source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    TransferSymlinkMode symlink_mode
) {
    StringTransferArchiveReader reader(&bytes);
    return import_path_from_reader(
        reader,
        source_type,
        absolute_path,
        overwrite_mode,
        create_parent,
        symlink_mode
    );
}

ImportSummary import_path_from_reader(
    TransferArchiveReader& reader,
    TransferSourceType source_type,
    const std::string& absolute_path,
    const std::string& overwrite_mode,
    bool create_parent,
    TransferSymlinkMode symlink_mode
) {
    ExportOptions options;
    options.symlink_mode = symlink_mode;
    validate_transfer_options(options);
    if (!is_absolute_path(absolute_path)) {
        throw TransferFailure(
            TransferRpcCode::PathNotAbsolute,
            "transfer path is not absolute"
        );
    }

    if (source_type == TransferSourceType::File) {
        return import_file_from_tar(
            reader,
            absolute_path,
            overwrite_mode,
            create_parent,
            options.symlink_mode
        );
    }
    if (source_type == TransferSourceType::Directory) {
        return import_directory_from_tar(
            reader,
            source_type,
            absolute_path,
            overwrite_mode,
            create_parent,
            options.symlink_mode
        );
    }
    if (source_type == TransferSourceType::Multiple) {
        return import_directory_from_tar(
            reader,
            source_type,
            absolute_path,
            overwrite_mode,
            create_parent,
            options.symlink_mode
        );
    }
    throw TransferFailure(
        TransferRpcCode::SourceUnsupported,
        "unsupported transfer source type"
    );
}
