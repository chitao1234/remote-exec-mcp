#include <set>
#include <string>
#include <vector>

#include "rpc/rpc_failures.h"
#include "transfer_archive.h"
#include "transfer_filesystem.h"
#include "transfer_import_materializer.h"
#include "transfer_import_plan.h"
#include "transfer_options.h"
#include "transfer_tar_codec.h"
#include "transfer_warning_codec.h"

namespace {

using namespace transfer_filesystem;
using namespace transfer_import_plan;
using namespace transfer_tar_codec;

TransferWarning skipped_symlink_warning(const std::string& path) {
    return TransferWarning{"transfer_skipped_symlink", "Skipped symlink transfer source entry `" + path + "`."};
}

enum SymlinkImportAction {
    SYMLINK_IMPORT_PRESERVE,
    SYMLINK_IMPORT_SKIP,
};

SymlinkImportAction symlink_import_action(TransferSymlinkMode symlink_mode, const std::string& error_path) {
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

void consume_file_archive_tail(TransferArchiveReader& reader,
                               std::vector<TransferWarning>* warnings,
                               const TransferLimitConfig& limits) {
    std::string pending_long_name;
    std::vector<char> block(TAR_BLOCK_SIZE);
    while (reader.read_exact_or_eof(block.data(), block.size())) {
        if (is_zero_block(block.data())) {
            require_archive_terminator(reader);
            return;
        }

        const TarHeaderView header = parse_header(block.data());

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name_from_reader(reader, header.size, limits);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        if (!is_transfer_summary_path(raw_path) || header.typeflag != '0') {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "file archive contains extra entries");
        }
        transfer_warning_codec::append_warnings(
            warnings,
            transfer_warning_codec::read_transfer_summary(
                read_limited_metadata_string(reader, header.size, limits, "truncated tar entry body")));
        skip_entry_padding(reader, header.size);
    }

    if (!pending_long_name.empty()) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "dangling GNU long name entry");
    }

    throw TransferFailure(TransferRpcCode::TransferFailed, "missing tar terminator");
}

ImportSummary import_file_archive(TransferArchiveReader& reader,
                                  const std::string& absolute_path,
                                  TransferOverwrite overwrite,
                                  bool create_parent,
                                  TransferSymlinkMode symlink_mode,
                                  const TransferLimitConfig& limits,
                                  const TransferPathAuthorizer& authorizer) {
    std::vector<char> block(TAR_BLOCK_SIZE);
    transfer_archive::read_exact_or_throw(reader, block.data(), block.size(), "archive is empty");
    const TarHeaderView header = parse_header(block.data());
    if (header.typeflag != '0' && header.typeflag != '2') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive entry is not a regular file");
    }
    if (header.path != SINGLE_FILE_ENTRY) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported,
                              "file archive entry path must be " + std::string(SINGLE_FILE_ENTRY));
    }

    ImportSummary summary = {TransferSourceType::File, 0, 1, 0, false, std::vector<TransferWarning>()};
    summary.replaced =
        prepare_destination_path(absolute_path, TransferSourceType::File, overwrite, create_parent, authorizer);

    if (header.typeflag == '2') {
        switch (symlink_import_action(symlink_mode, std::string(SINGLE_FILE_ENTRY))) {
        case SYMLINK_IMPORT_SKIP:
            summary.warnings.push_back(skipped_symlink_warning(std::string(SINGLE_FILE_ENTRY)));
            summary.files_copied = 0;
            break;
        case SYMLINK_IMPORT_PRESERVE:
            ensure_not_existing_symlink(absolute_path);
            transfer_import_materializer::write_validated_symlink(header.link_name, absolute_path, authorizer);
            break;
        }
        transfer_archive::skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
    } else {
        ensure_transfer_entry_within_limits(header.size, 0U, limits);
        ensure_not_existing_symlink(absolute_path);
        transfer_import_materializer::write_entry_body_to_file(
            reader, absolute_path, header.size, header.mode, authorizer);
        skip_entry_padding(reader, header.size);
        summary.bytes_copied = header.size;
    }

    consume_file_archive_tail(reader, &summary.warnings, limits);
    return summary;
}

ImportSummary import_tree_archive(TransferArchiveReader& reader,
                                  TransferSourceType source_type,
                                  const std::string& absolute_path,
                                  TransferOverwrite overwrite,
                                  bool create_parent,
                                  TransferSymlinkMode symlink_mode,
                                  const TransferLimitConfig& limits,
                                  const TransferPathAuthorizer& authorizer) {
    ImportSummary summary = {source_type, 0, 0, 1, false, std::vector<TransferWarning>()};
    summary.replaced = prepare_destination_path(absolute_path, source_type, overwrite, create_parent, authorizer);
    make_directory_if_missing(absolute_path);

    std::set<std::string> replaced_units;
    std::string pending_long_name;
    std::vector<char> block(TAR_BLOCK_SIZE);
    while (reader.read_exact_or_eof(block.data(), block.size())) {
        if (is_zero_block(block.data())) {
            require_archive_terminator(reader);
            return summary;
        }

        const TarHeaderView header = parse_header(block.data());

        if (header.typeflag == 'L') {
            pending_long_name = read_gnu_long_name_from_reader(reader, header.size, limits);
            continue;
        }

        const std::string raw_path = pending_long_name.empty() ? header.path : pending_long_name;
        pending_long_name.clear();
        const std::string relative_path = validate_relative_archive_path(raw_path);
        if (is_transfer_summary_path(relative_path)) {
            if (header.typeflag != '0') {
                throw TransferFailure(TransferRpcCode::SourceUnsupported,
                                      "transfer summary archive entry is not a regular file");
            }
            transfer_warning_codec::append_warnings(
                &summary.warnings,
                transfer_warning_codec::read_transfer_summary(
                    read_limited_metadata_string(reader, header.size, limits, "truncated tar entry body")));
            skip_entry_padding(reader, header.size);
            continue;
        }

        const std::string output_path = materialize_archive_path(absolute_path, relative_path);
        if (source_type == TransferSourceType::Multiple && overwrite == TransferOverwrite::Replace) {
            const std::string unit = top_level_archive_component(relative_path);
            if (!unit.empty() && replaced_units.insert(unit).second) {
                replace_existing_path(join_path(absolute_path, unit), authorizer);
            }
        }

        if (header.typeflag == '5') {
            if (relative_path != ".") {
                transfer_import_materializer::materialize_directory_entry(
                    absolute_path, relative_path, output_path, authorizer);
                summary.directories_copied += 1;
            }
            transfer_archive::skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
            continue;
        }

        if (header.typeflag == '2') {
            switch (symlink_import_action(symlink_mode, "")) {
            case SYMLINK_IMPORT_SKIP:
                summary.warnings.push_back(skipped_symlink_warning(relative_path));
                transfer_archive::skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
                continue;
            case SYMLINK_IMPORT_PRESERVE:
                break;
            }
            if (relative_path == ".") {
                throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink entry cannot target root");
            }
            transfer_import_materializer::materialize_symlink_entry(
                header.link_name, absolute_path, relative_path, output_path, authorizer);
            summary.files_copied += 1;
            transfer_archive::skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
            continue;
        }

        if (header.typeflag != '0') {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive contains unsupported entry");
        }
        if (relative_path == ".") {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive file entry cannot target root");
        }

        ensure_transfer_entry_within_limits(header.size, summary.bytes_copied, limits);
        transfer_import_materializer::materialize_file_entry(
            reader, absolute_path, relative_path, output_path, header.size, header.mode, authorizer);
        skip_entry_padding(reader, header.size);
        summary.bytes_copied += header.size;
        summary.files_copied += 1;
    }

    if (!pending_long_name.empty()) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "dangling GNU long name entry");
    }

    throw TransferFailure(TransferRpcCode::TransferFailed, "missing tar terminator");
}

} // namespace

ImportSummary import_path(const std::string& bytes,
                          TransferSourceType source_type,
                          const std::string& absolute_path,
                          TransferOverwrite overwrite,
                          bool create_parent,
                          TransferSymlinkMode symlink_mode,
                          const TransferLimitConfig& limits,
                          const TransferPathAuthorizer& authorizer) {
    transfer_archive::StringArchiveReader reader(&bytes);
    return import_path_from_reader(
        reader, source_type, absolute_path, overwrite, create_parent, symlink_mode, limits, authorizer);
}

ImportSummary import_path_from_reader(TransferArchiveReader& reader,
                                      TransferSourceType source_type,
                                      const std::string& absolute_path,
                                      TransferOverwrite overwrite,
                                      bool create_parent,
                                      TransferSymlinkMode symlink_mode,
                                      const TransferLimitConfig& limits,
                                      const TransferPathAuthorizer& authorizer) {
    transfer_options::ExportOptions options;
    options.symlink_mode = symlink_mode;
    transfer_options::validate_transfer_options(options);
    if (!is_absolute_path(absolute_path)) {
        throw TransferFailure(TransferRpcCode::PathNotAbsolute, "transfer path is not absolute");
    }

    if (source_type == TransferSourceType::File) {
        return import_file_archive(
            reader, absolute_path, overwrite, create_parent, options.symlink_mode, limits, authorizer);
    }
    if (source_type == TransferSourceType::Directory) {
        return import_tree_archive(
            reader, source_type, absolute_path, overwrite, create_parent, options.symlink_mode, limits, authorizer);
    }
    if (source_type == TransferSourceType::Multiple) {
        return import_tree_archive(
            reader, source_type, absolute_path, overwrite, create_parent, options.symlink_mode, limits, authorizer);
    }
    throw TransferFailure(TransferRpcCode::SourceUnsupported, "unsupported transfer source type");
}
