#include <atomic>
#include <algorithm>
#include <cctype>
#include <cstdio>
#include <fstream>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#include "platform/path_utils.h"
#include "rpc/rpc_failures.h"
#include "platform/scoped_file.h"
#include "core/stdio_retry.h"
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

std::string unique_atomic_write_temp_path(const std::string& path) {
    static std::atomic<unsigned long> next_suffix(1UL);

    std::ostringstream out;
    out << path << ".tmp." << next_suffix.fetch_add(1UL);
    return out.str();
}

enum ImportPlanEntryKind {
    IMPORT_PLAN_DIRECTORY,
    IMPORT_PLAN_FILE,
    IMPORT_PLAN_SYMLINK,
};

struct ImportPlanEntry {
    ImportPlanEntryKind kind;
    std::string relative_path;
    std::string body;
    std::uint64_t mode;
    std::string symlink_target;
    std::string temp_path;
};

struct ImportPlan {
    ImportSummary summary;
    std::vector<ImportPlanEntry> entries;
};

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
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path must be relative");
    }
    if (normalized.size() >= 2 && std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
        normalized[1] == ':') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path must be relative");
    }
    if (normalized.rfind("//", 0) == 0) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path must be relative");
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    std::vector<std::string> cleaned;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        const std::string& part = parts[i];
        if (part.empty()) {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path contains empty component");
        }
        if (part == "." || part == "..") {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path escapes destination");
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

std::string validate_relative_symlink_target(const std::string& raw_target) {
    const std::string normalized = normalize_archive_separators(raw_target);
    if (normalized.empty() || normalized[0] == '/' || normalized.rfind("//", 0) == 0) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink target must be relative");
    }
    if (normalized.size() >= 2 && std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
        normalized[1] == ':') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink target must be relative");
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    for (std::size_t i = 0; i < parts.size(); ++i) {
        if (parts[i].empty() || parts[i] == "." || parts[i] == "..") {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink target escapes destination");
        }
    }
    return normalized;
}

std::string materialize_archive_path(const std::string& destination_root, const std::string& relative_archive_path) {
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

void ensure_no_existing_symlink_in_path(const std::string& destination_root, const std::string& relative_archive_path) {
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

std::string resolved_symlink_target_path(const std::string& symlink_path, const std::string& relative_target) {
    const std::string parent = path_utils::parent_directory(symlink_path);
    if (parent.empty()) {
        return relative_target;
    }
    return join_path(parent, relative_target);
}

void authorize_path_if_present(const TransferPathAuthorizer& authorizer, const std::string& path) {
    if (authorizer) {
        authorizer(path);
    }
}

void write_validated_symlink(const std::string& raw_target,
                             const std::string& output_path,
                             const TransferPathAuthorizer& authorizer) {
    const std::string target = validate_relative_symlink_target(raw_target);
    authorize_path_if_present(authorizer, output_path);
    authorize_path_if_present(authorizer, resolved_symlink_target_path(output_path, target));
    write_symlink(target, output_path);
}

class StringTransferArchiveReader : public TransferArchiveReader {
public:
    explicit StringTransferArchiveReader(const std::string* archive) : archive_(archive), offset_(0) {}

    bool read_exact_or_eof(char* data, std::size_t size) {
        if (size == 0U) {
            return true;
        }
        if (offset_ >= archive_->size()) {
            return false;
        }
        if (archive_->size() - offset_ < size) {
            throw TransferFailure(TransferRpcCode::TransferFailed, "truncated transfer body");
        }
        std::copy(archive_->data() + offset_, archive_->data() + offset_ + size, data);
        offset_ += size;
        return true;
    }

private:
    const std::string* archive_;
    std::size_t offset_;
};

void read_exact_or_throw(TransferArchiveReader& reader,
                         char* data,
                         std::size_t size,
                         const std::string& error_message) {
    if (!reader.read_exact_or_eof(data, size)) {
        throw TransferFailure(TransferRpcCode::TransferFailed, error_message);
    }
}

std::string read_exact_string(TransferArchiveReader& reader, std::uint64_t size, const std::string& error_message) {
    ensure_u64_fits_size_t(size, "tar entry size");
    std::string body(static_cast<std::size_t>(size), '\0');
    if (!body.empty()) {
        read_exact_or_throw(reader, &body[0], body.size(), error_message);
    }
    return body;
}

std::string read_limited_metadata_string(TransferArchiveReader& reader,
                                         std::uint64_t size,
                                         const TransferLimitConfig& limits,
                                         const std::string& error_message) {
    ensure_transfer_entry_within_limits(size, 0U, limits);
    return read_exact_string(reader, size, error_message);
}

void skip_exact(TransferArchiveReader& reader, std::uint64_t size, const std::string& error_message) {
    char buffer[8192];
    std::uint64_t remaining = size;
    while (remaining > 0U) {
        const std::size_t requested = remaining < sizeof(buffer) ? static_cast<std::size_t>(remaining) : sizeof(buffer);
        read_exact_or_throw(reader, buffer, requested, error_message);
        remaining -= static_cast<std::uint64_t>(requested);
    }
}

void require_archive_terminator(TransferArchiveReader& reader) {
    std::vector<char> terminator(TAR_BLOCK_SIZE);
    read_exact_or_throw(reader, terminator.data(), terminator.size(), "truncated tar terminator");
    if (!is_zero_block(terminator.data())) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "invalid tar terminator");
    }

    while (reader.read_exact_or_eof(terminator.data(), terminator.size())) {
        if (!is_zero_block(terminator.data())) {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "trailing data after tar terminator");
        }
    }
}

std::uint64_t entry_padding(std::uint64_t size) {
    const std::uint64_t remainder = size % TAR_BLOCK_SIZE;
    return remainder == 0U ? 0U : static_cast<std::uint64_t>(TAR_BLOCK_SIZE) - remainder;
}

std::uint64_t entry_body_with_padding(std::uint64_t size) {
    if (size > std::numeric_limits<std::uint64_t>::max() - entry_padding(size)) {
        throw TransferFailure(TransferRpcCode::TransferFailed, "tar entry size is too large");
    }
    return size + entry_padding(size);
}

void skip_entry_padding(TransferArchiveReader& reader, std::uint64_t size) {
    skip_exact(reader, entry_padding(size), "truncated tar entry body");
}

std::string
read_gnu_long_name_from_reader(TransferArchiveReader& reader, std::uint64_t size, const TransferLimitConfig& limits) {
    std::string value = read_limited_metadata_string(reader, size, limits, "truncated tar entry body");
    skip_entry_padding(reader, size);
    while (!value.empty() && value[value.size() - 1] == '\0') {
        value.erase(value.size() - 1);
    }
    return value;
}

void write_body_to_file_atomic(const std::string& path,
                               const std::string& body,
                               std::uint64_t mode,
                               const TransferPathAuthorizer& authorizer,
                               const std::string& planned_temp_path) {
    authorize_path_if_present(authorizer, path);
    const std::string temp_path = planned_temp_path.empty() ? unique_atomic_write_temp_path(path) : planned_temp_path;
    authorize_path_if_present(authorizer, temp_path);
    ScopedFile output(path_utils::open_file(temp_path, "wb"));
    if (!output.valid()) {
        throw std::runtime_error("unable to write destination file");
    }
    try {
        if (!body.empty() && !stdio_retry::fwrite_all(output.get(), body.data(), body.size())) {
            throw std::runtime_error("unable to write destination file");
        }
#ifndef _WIN32
        if ((mode & 0111U) != 0U) {
            if (!path_utils::add_executable_bits(output.get())) {
                throw std::runtime_error("unable to update destination file mode");
            }
        }
#else
        (void)mode;
#endif
        if (output.close() != 0) {
            throw std::runtime_error("unable to write destination file");
        }
        if (!path_utils::rename_path(temp_path, path)) {
            (void)path_utils::remove_path(temp_path);
            throw std::runtime_error("unable to rename temporary destination file");
        }
    } catch (...) {
        (void)path_utils::remove_path(temp_path);
        throw;
    }
}

ImportPlanEntry directory_plan_entry(const std::string& relative_path) {
    ImportPlanEntry entry;
    entry.kind = IMPORT_PLAN_DIRECTORY;
    entry.relative_path = relative_path;
    entry.mode = 0U;
    return entry;
}

ImportPlanEntry file_plan_entry(const std::string& relative_path, std::string body, std::uint64_t mode) {
    ImportPlanEntry entry;
    entry.kind = IMPORT_PLAN_FILE;
    entry.relative_path = relative_path;
    entry.body.swap(body);
    entry.mode = mode;
    return entry;
}

ImportPlanEntry symlink_plan_entry(const std::string& relative_path, const std::string& target) {
    ImportPlanEntry entry;
    entry.kind = IMPORT_PLAN_SYMLINK;
    entry.relative_path = relative_path;
    entry.mode = 0U;
    entry.symlink_target = target;
    return entry;
}

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
        append_warnings(warnings,
                        read_transfer_summary(
                            read_limited_metadata_string(reader, header.size, limits, "truncated tar entry body")));
        skip_entry_padding(reader, header.size);
    }

    if (!pending_long_name.empty()) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "dangling GNU long name entry");
    }

    throw TransferFailure(TransferRpcCode::TransferFailed, "missing tar terminator");
}

ImportPlan plan_file_import(TransferArchiveReader& reader,
                            TransferSymlinkMode symlink_mode,
                            const TransferLimitConfig& limits) {
    std::vector<char> block(TAR_BLOCK_SIZE);
    read_exact_or_throw(reader, block.data(), block.size(), "archive is empty");
    const TarHeaderView header = parse_header(block.data());
    if (header.typeflag != '0' && header.typeflag != '2') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive entry is not a regular file");
    }
    if (header.path != SINGLE_FILE_ENTRY) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported,
                              "file archive entry path must be " + std::string(SINGLE_FILE_ENTRY));
    }

    ImportPlan plan;
    plan.summary = ImportSummary{
        TransferSourceType::File,
        0,
        1,
        0,
        false,
        std::vector<TransferWarning>(),
    };

    if (header.typeflag == '2') {
        switch (symlink_import_action(symlink_mode, std::string(SINGLE_FILE_ENTRY))) {
        case SYMLINK_IMPORT_SKIP:
            plan.summary.warnings.push_back(skipped_symlink_warning(std::string(SINGLE_FILE_ENTRY)));
            plan.summary.files_copied = 0;
            break;
        case SYMLINK_IMPORT_PRESERVE:
            plan.entries.push_back(symlink_plan_entry(".", validate_relative_symlink_target(header.link_name)));
            break;
        }
        skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
    } else {
        ensure_transfer_entry_within_limits(header.size, 0U, limits);
        std::string body = read_exact_string(reader, header.size, "truncated tar entry body");
        skip_entry_padding(reader, header.size);
        plan.entries.push_back(file_plan_entry(".", body, header.mode));
        plan.summary.bytes_copied = header.size;
    }

    consume_file_archive_tail(reader, &plan.summary.warnings, limits);

    return plan;
}

ImportPlan plan_directory_import(TransferArchiveReader& reader,
                                 TransferSourceType source_type,
                                 TransferSymlinkMode symlink_mode,
                                 const TransferLimitConfig& limits) {
    ImportPlan plan;
    plan.summary = {source_type, 0, 0, 1, false, std::vector<TransferWarning>()};
    std::string pending_long_name;
    std::vector<char> block(TAR_BLOCK_SIZE);

    while (reader.read_exact_or_eof(block.data(), block.size())) {
        if (is_zero_block(block.data())) {
            require_archive_terminator(reader);
            return plan;
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
            append_warnings(&plan.summary.warnings,
                            read_transfer_summary(
                                read_limited_metadata_string(reader, header.size, limits, "truncated tar entry body")));
            skip_entry_padding(reader, header.size);
            continue;
        }

        if (header.typeflag == '5') {
            if (relative_path != ".") {
                plan.entries.push_back(directory_plan_entry(relative_path));
                plan.summary.directories_copied += 1;
            }
            skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
            continue;
        }

        if (header.typeflag == '2') {
            switch (symlink_import_action(symlink_mode, "")) {
            case SYMLINK_IMPORT_SKIP:
                plan.summary.warnings.push_back(skipped_symlink_warning(relative_path));
                skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
                continue;
            case SYMLINK_IMPORT_PRESERVE:
                break;
            }
            if (relative_path == ".") {
                throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink entry cannot target root");
            }
            plan.entries.push_back(symlink_plan_entry(relative_path, validate_relative_symlink_target(header.link_name)));
            plan.summary.files_copied += 1;
            skip_exact(reader, entry_body_with_padding(header.size), "truncated tar entry body");
            continue;
        }

        if (header.typeflag != '0') {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive contains unsupported entry");
        }
        if (relative_path == ".") {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive file entry cannot target root");
        }

        ensure_transfer_entry_within_limits(header.size, plan.summary.bytes_copied, limits);
        std::string body = read_exact_string(reader, header.size, "truncated tar entry body");
        skip_entry_padding(reader, header.size);
        plan.entries.push_back(file_plan_entry(relative_path, body, header.mode));
        plan.summary.bytes_copied += header.size;
        plan.summary.files_copied += 1;
    }

    if (!pending_long_name.empty()) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "dangling GNU long name entry");
    }

    throw TransferFailure(TransferRpcCode::TransferFailed, "missing tar terminator");
}

void authorize_materialized_relative_path(const std::string& destination_root,
                                          const std::string& relative_path,
                                          const TransferPathAuthorizer& authorizer) {
    if (!authorizer) {
        return;
    }
    if (relative_path == ".") {
        authorizer(destination_root);
        return;
    }

    const std::vector<std::string> parts = split_archive_path(relative_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = join_path(path, parts[i]);
        authorizer(path);
    }
}

void authorize_import_plan(const ImportPlan& plan,
                           const std::string& absolute_path,
                           const TransferPathAuthorizer& authorizer) {
    if (!authorizer) {
        return;
    }

    authorizer(absolute_path);
    for (std::size_t i = 0; i < plan.entries.size(); ++i) {
        const ImportPlanEntry& entry = plan.entries[i];
        const std::string output_path = materialize_archive_path(absolute_path, entry.relative_path);
        authorize_materialized_relative_path(absolute_path, entry.relative_path, authorizer);
        if (entry.kind == IMPORT_PLAN_SYMLINK) {
            authorizer(resolved_symlink_target_path(output_path, entry.symlink_target));
        }
    }
}

void prepare_import_plan_commit_paths(ImportPlan* plan,
                                      const std::string& absolute_path,
                                      const TransferPathAuthorizer& authorizer) {
    authorize_import_plan(*plan, absolute_path, authorizer);

    for (std::size_t i = 0; i < plan->entries.size(); ++i) {
        ImportPlanEntry& entry = plan->entries[i];
        if (entry.kind != IMPORT_PLAN_FILE) {
            continue;
        }
        const std::string output_path = materialize_archive_path(absolute_path, entry.relative_path);
        entry.temp_path = unique_atomic_write_temp_path(output_path);
        authorize_path_if_present(authorizer, entry.temp_path);
    }
}

ImportSummary execute_file_import_plan(const ImportPlan& plan,
                                       const std::string& absolute_path,
                                       TransferOverwrite overwrite,
                                       bool create_parent,
                                       const TransferPathAuthorizer& authorizer) {
    ImportSummary summary = plan.summary;
    summary.replaced =
        prepare_destination_path(absolute_path, TransferSourceType::File, overwrite, create_parent, authorizer);
    if (plan.entries.empty()) {
        return summary;
    }

    ensure_not_existing_symlink(absolute_path);
    const ImportPlanEntry& entry = plan.entries[0];
    if (entry.kind == IMPORT_PLAN_FILE) {
        write_body_to_file_atomic(absolute_path, entry.body, entry.mode, authorizer, entry.temp_path);
        return summary;
    }
    if (entry.kind == IMPORT_PLAN_SYMLINK) {
        write_validated_symlink(entry.symlink_target, absolute_path, authorizer);
        return summary;
    }

    throw TransferFailure(TransferRpcCode::SourceUnsupported, "file archive contains unsupported entry");
}

ImportSummary execute_directory_import_plan(const ImportPlan& plan,
                                            const std::string& absolute_path,
                                            TransferSourceType source_type,
                                            TransferOverwrite overwrite,
                                            bool create_parent,
                                            const TransferPathAuthorizer& authorizer) {
    ImportSummary summary = plan.summary;
    summary.replaced = prepare_destination_path(absolute_path, source_type, overwrite, create_parent, authorizer);
    make_directory_if_missing(absolute_path);

    for (std::size_t i = 0; i < plan.entries.size(); ++i) {
        const ImportPlanEntry& entry = plan.entries[i];
        const std::string output_path = materialize_archive_path(absolute_path, entry.relative_path);

        if (entry.kind == IMPORT_PLAN_DIRECTORY) {
            ensure_no_existing_symlink_in_path(absolute_path, entry.relative_path);
            ensure_parent_directory(output_path, true);
            make_directory_if_missing(output_path);
            continue;
        }

        if (entry.kind == IMPORT_PLAN_SYMLINK) {
            ensure_no_existing_symlink_in_path(absolute_path, entry.relative_path);
            write_validated_symlink(entry.symlink_target, output_path, authorizer);
            continue;
        }

        if (entry.kind == IMPORT_PLAN_FILE) {
            ensure_no_existing_symlink_in_path(absolute_path, entry.relative_path);
            ensure_parent_directory(output_path, true);
            write_body_to_file_atomic(output_path, entry.body, entry.mode, authorizer, entry.temp_path);
            continue;
        }
    }

    return summary;
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
    StringTransferArchiveReader reader(&bytes);
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
    ExportOptions options;
    options.symlink_mode = symlink_mode;
    validate_transfer_options(options);
    if (!is_absolute_path(absolute_path)) {
        throw TransferFailure(TransferRpcCode::PathNotAbsolute, "transfer path is not absolute");
    }

    if (source_type == TransferSourceType::File) {
        ImportPlan plan = plan_file_import(reader, options.symlink_mode, limits);
        prepare_import_plan_commit_paths(&plan, absolute_path, authorizer);
        return execute_file_import_plan(plan, absolute_path, overwrite, create_parent, authorizer);
    }
    if (source_type == TransferSourceType::Directory) {
        ImportPlan plan = plan_directory_import(reader, source_type, options.symlink_mode, limits);
        prepare_import_plan_commit_paths(&plan, absolute_path, authorizer);
        return execute_directory_import_plan(plan, absolute_path, source_type, overwrite, create_parent, authorizer);
    }
    if (source_type == TransferSourceType::Multiple) {
        ImportPlan plan = plan_directory_import(reader, source_type, options.symlink_mode, limits);
        prepare_import_plan_commit_paths(&plan, absolute_path, authorizer);
        return execute_directory_import_plan(plan, absolute_path, source_type, overwrite, create_parent, authorizer);
    }
    throw TransferFailure(TransferRpcCode::SourceUnsupported, "unsupported transfer source type");
}
