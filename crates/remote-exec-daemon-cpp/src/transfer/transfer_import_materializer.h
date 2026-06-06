#pragma once

#include <cstdint>
#include <string>

#include "transfer/transfer_ops.h"

namespace transfer_import_materializer {

void authorize_materialized_relative_path(const std::string& destination_root,
                                          const std::string& relative_path,
                                          const TransferPathAuthorizer& authorizer);
void ensure_no_existing_symlink_in_path(const std::string& destination_root, const std::string& relative_archive_path);
void write_validated_symlink(const std::string& raw_target,
                             const std::string& output_path,
                             const TransferPathAuthorizer& authorizer);
void write_entry_body_to_file(TransferArchiveReader& reader,
                              const std::string& path,
                              std::uint64_t size,
                              std::uint64_t mode,
                              const TransferPathAuthorizer& authorizer);
void materialize_directory_entry(const std::string& destination_root,
                                 const std::string& relative_path,
                                 const std::string& output_path,
                                 const TransferPathAuthorizer& authorizer);
void materialize_symlink_entry(const std::string& raw_target,
                               const std::string& destination_root,
                               const std::string& relative_path,
                               const std::string& output_path,
                               const TransferPathAuthorizer& authorizer);
void materialize_file_entry(TransferArchiveReader& reader,
                            const std::string& destination_root,
                            const std::string& relative_path,
                            const std::string& output_path,
                            std::uint64_t size,
                            std::uint64_t mode,
                            const TransferPathAuthorizer& authorizer);

} // namespace transfer_import_materializer
