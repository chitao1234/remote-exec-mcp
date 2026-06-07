#pragma once

#include <string>
#include <vector>

#include "transfer/transfer_archive_io.h"
#include "transfer/transfer_types.h"

ExportedPayload export_path(const std::string& absolute_path,
                            TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                            const std::vector<std::string>& exclude = std::vector<std::string>(),
                            const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
TransferSourceType export_path_source_type(const std::string& absolute_path,
                                           TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve);
TransferSourceType export_path_to_sink(TransferArchiveSink& sink,
                                       const std::string& absolute_path,
                                       TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                                       const std::vector<std::string>& exclude = std::vector<std::string>(),
                                       const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
void export_path_to_sink_as(TransferArchiveSink& sink,
                            const std::string& absolute_path,
                            TransferSourceType source_type,
                            TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                            const std::vector<std::string>& exclude = std::vector<std::string>(),
                            const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
PathInfo path_info(const std::string& absolute_path);
ImportSummary import_path(const std::string& bytes,
                          TransferSourceType source_type,
                          const std::string& absolute_path,
                          TransferOverwrite overwrite,
                          bool create_parent,
                          TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                          const TransferLimitConfig& limits = default_transfer_limit_config(),
                          const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
ImportSummary import_path_from_reader(TransferArchiveReader& reader,
                                      TransferSourceType source_type,
                                      const std::string& absolute_path,
                                      TransferOverwrite overwrite,
                                      bool create_parent,
                                      TransferSymlinkMode symlink_mode = TransferSymlinkMode::Preserve,
                                      const TransferLimitConfig& limits = default_transfer_limit_config(),
                                      const TransferPathAuthorizer& authorizer = TransferPathAuthorizer());
