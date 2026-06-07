#pragma once

#include <string>
#include <vector>

#include "http/http_helpers.h"
#include "transfer/transfer_types.h"

struct TransferImportMetadata {
    std::string destination_path;
    TransferOverwrite overwrite;
    bool create_parent;
    TransferSourceType source_type;
    std::string compression;
    TransferSymlinkMode symlink_mode;
};

void require_uncompressed_transfer(const std::string& compression);
void require_transfer_stream_version(const HttpRequest& request);
void require_transfer_stream_content_type(const HttpRequest& request);
TransferImportMetadata parse_transfer_import_metadata(const HttpRequest& request);
void write_transfer_export_headers(HttpResponse& response, const ExportedPayload& payload);
Json transfer_warnings_json(const std::vector<TransferWarning>& warnings);
Json transfer_summary_json(const ImportSummary& summary);
void log_transfer_import_summary(const std::string& destination_path, const ImportSummary& summary);
