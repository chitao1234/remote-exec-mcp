#include <sstream>
#include <string>

#include "logging.h"
#include "rpc_failures.h"
#include "transfer_http_codec.h"

void require_uncompressed_transfer(const std::string& compression) {
    if (!compression.empty() && compression != "none") {
        throw TransferFailure(
            TransferRpcCode::CompressionUnsupported,
            "this daemon does not support transfer compression"
        );
    }
}

TransferImportMetadata parse_transfer_import_metadata(const HttpRequest& request) {
    TransferImportMetadata metadata;
    metadata.destination_path = request.header("x-remote-exec-destination-path");
    metadata.overwrite = request.header("x-remote-exec-overwrite");
    metadata.create_parent = request.header("x-remote-exec-create-parent") == "true";
    metadata.source_type = request.header("x-remote-exec-source-type");
    metadata.compression = request.header("x-remote-exec-compression");
    metadata.symlink_mode = request.header("x-remote-exec-symlink-mode");
    if (metadata.symlink_mode.empty()) {
        metadata.symlink_mode = "preserve";
    }
    return metadata;
}

void write_transfer_export_headers(HttpResponse& response, const ExportedPayload& payload) {
    response.headers["Content-Type"] = "application/octet-stream";
    response.headers["x-remote-exec-source-type"] = payload.source_type;
    response.headers["x-remote-exec-compression"] = "none";
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

Json transfer_summary_json(const ImportSummary& summary) {
    return Json{
        {"source_type", summary.source_type},
        {"bytes_copied", summary.bytes_copied},
        {"files_copied", summary.files_copied},
        {"directories_copied", summary.directories_copied},
        {"replaced", summary.replaced},
        {"warnings", transfer_warnings_json(summary.warnings)},
    };
}

void log_transfer_import_summary(const std::string& destination_path, const ImportSummary& summary) {
    std::ostringstream message;
    message << "transfer/import destination=`" << destination_path
            << "` bytes_copied=" << summary.bytes_copied
            << " files_copied=" << summary.files_copied
            << " directories_copied=" << summary.directories_copied
            << " replaced=" << (summary.replaced ? "true" : "false");
    log_message(LOG_INFO, "server", message.str());
}
