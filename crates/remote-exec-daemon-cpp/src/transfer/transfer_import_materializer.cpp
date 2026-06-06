#include <cstdio>
#include <stdexcept>
#include <string>
#include <vector>

#include "core/stdio_retry.h"
#include "platform/path_utils.h"
#include "platform/scoped_file.h"
#include "transfer_archive.h"
#include "transfer_filesystem.h"
#include "transfer_import_materializer.h"
#include "transfer_import_plan.h"

namespace transfer_import_materializer {

namespace {

void authorize_path_if_present(const TransferPathAuthorizer& authorizer, const std::string& path) {
    if (authorizer) {
        authorizer(path);
    }
}

} // namespace

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

    const std::vector<std::string> parts = transfer_import_plan::split_archive_path(relative_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = transfer_filesystem::join_path(path, parts[i]);
        authorizer(path);
    }
}

void ensure_no_existing_symlink_in_path(const std::string& destination_root, const std::string& relative_archive_path) {
    transfer_filesystem::ensure_not_existing_symlink(destination_root);
    if (relative_archive_path == ".") {
        return;
    }

    const std::vector<std::string> parts = transfer_import_plan::split_archive_path(relative_archive_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = transfer_filesystem::join_path(path, parts[i]);
        transfer_filesystem::ensure_not_existing_symlink(path);
    }
}

void write_validated_symlink(const std::string& raw_target,
                             const std::string& output_path,
                             const TransferPathAuthorizer& authorizer) {
    const std::string target = transfer_import_plan::validate_relative_symlink_target(raw_target);
    authorize_path_if_present(authorizer, output_path);
    authorize_path_if_present(authorizer, transfer_import_plan::resolved_symlink_target_path(output_path, target));
    transfer_filesystem::write_symlink(target, output_path);
}

void write_entry_body_to_file(TransferArchiveReader& reader,
                              const std::string& path,
                              std::uint64_t size,
                              std::uint64_t mode,
                              const TransferPathAuthorizer& authorizer) {
    authorize_path_if_present(authorizer, path);
    ScopedFile output(path_utils::open_file(path, "wb"));
    if (!output.valid()) {
        throw std::runtime_error("unable to write destination file");
    }

    char buffer[8192];
    std::uint64_t remaining = size;
    while (remaining > 0U) {
        const std::size_t requested = remaining < sizeof(buffer) ? static_cast<std::size_t>(remaining) : sizeof(buffer);
        transfer_archive::read_exact_or_throw(reader, buffer, requested, "truncated tar entry body");
        if (!stdio_retry::fwrite_all(output.get(), buffer, requested)) {
            throw std::runtime_error("unable to write destination file");
        }
        remaining -= static_cast<std::uint64_t>(requested);
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
}

void materialize_directory_entry(const std::string& destination_root,
                                 const std::string& relative_path,
                                 const std::string& output_path,
                                 const TransferPathAuthorizer& authorizer) {
    authorize_materialized_relative_path(destination_root, relative_path, authorizer);
    ensure_no_existing_symlink_in_path(destination_root, relative_path);
    transfer_filesystem::ensure_parent_directory(output_path, true);
    transfer_filesystem::make_directory_if_missing(output_path);
}

void materialize_symlink_entry(const std::string& raw_target,
                               const std::string& destination_root,
                               const std::string& relative_path,
                               const std::string& output_path,
                               const TransferPathAuthorizer& authorizer) {
    authorize_materialized_relative_path(destination_root, relative_path, authorizer);
    ensure_no_existing_symlink_in_path(destination_root, relative_path);
    write_validated_symlink(raw_target, output_path, authorizer);
}

void materialize_file_entry(TransferArchiveReader& reader,
                            const std::string& destination_root,
                            const std::string& relative_path,
                            const std::string& output_path,
                            std::uint64_t size,
                            std::uint64_t mode,
                            const TransferPathAuthorizer& authorizer) {
    authorize_materialized_relative_path(destination_root, relative_path, authorizer);
    ensure_no_existing_symlink_in_path(destination_root, relative_path);
    transfer_filesystem::ensure_parent_directory(output_path, true);
    write_entry_body_to_file(reader, output_path, size, mode, authorizer);
}

} // namespace transfer_import_materializer
