#include <initializer_list>
#include <string>

#include "base64_codec.h"
#include "logging.h"
#include "rpc_failures.h"
#include "server_contract.h"
#include "transfer_http_codec.h"

namespace {

std::string missing_header_message(const char* name) {
    return std::string("missing header `") + name + "`";
}

std::string invalid_header_message(const char* name, const std::string& detail) {
    return std::string("invalid header `") + name + "`: " + detail;
}

std::string required_header(const HttpRequest& request, const char* name) {
    const std::map<std::string, std::string>::const_iterator it = request.headers.find(name);
    if (it == request.headers.end()) {
        throw TransferFailure(TransferRpcCode::BadRequest, missing_header_message(name));
    }
    return it->second;
}

std::string optional_header_or(const HttpRequest& request, const char* name, const std::string& fallback) {
    const std::map<std::string, std::string>::const_iterator it = request.headers.find(name);
    if (it == request.headers.end()) {
        return fallback;
    }
    return it->second;
}

void require_one_of(const char* name, const std::string& value, std::initializer_list<const char*> allowed_values) {
    for (std::initializer_list<const char*>::const_iterator it = allowed_values.begin(); it != allowed_values.end();
         ++it) {
        if (value == *it) {
            return;
        }
    }
    throw TransferFailure(TransferRpcCode::BadRequest,
                          invalid_header_message(name, "unsupported value `" + value + "`"));
}

bool parse_create_parent(const std::string& value) {
    if (value == "true") {
        return true;
    }
    if (value == "false") {
        return false;
    }
    throw TransferFailure(TransferRpcCode::BadRequest,
                          invalid_header_message(
                              server_contract::TRANSFER_CREATE_PARENT_HEADER, "expected `true` or `false`"));
}

std::string decode_destination_path_header(const std::string& encoded) {
    try {
        return base64_decode_bytes(encoded);
    } catch (const std::runtime_error& ex) {
        throw TransferFailure(
            TransferRpcCode::BadRequest,
            invalid_header_message(server_contract::TRANSFER_DESTINATION_PATH_HEADER,
                                   "expected base64-encoded UTF-8 path: " + std::string(ex.what())));
    }
}

} // namespace

void require_uncompressed_transfer(const std::string& compression) {
    if (!compression.empty() && compression != "none") {
        throw TransferFailure(TransferRpcCode::CompressionUnsupported,
                              "this daemon does not support transfer compression");
    }
}

TransferImportMetadata parse_transfer_import_metadata(const HttpRequest& request) {
    TransferImportMetadata metadata;
    metadata.destination_path =
        decode_destination_path_header(required_header(request, server_contract::TRANSFER_DESTINATION_PATH_HEADER));
    const std::string overwrite = required_header(request, server_contract::TRANSFER_OVERWRITE_HEADER);
    if (!parse_transfer_overwrite_wire_value(overwrite, &metadata.overwrite)) {
        throw TransferFailure(TransferRpcCode::BadRequest,
                              invalid_header_message(server_contract::TRANSFER_OVERWRITE_HEADER,
                                                     "unsupported value `" + overwrite + "`"));
    }
    metadata.create_parent =
        parse_create_parent(required_header(request, server_contract::TRANSFER_CREATE_PARENT_HEADER));
    const std::string source_type = required_header(request, server_contract::TRANSFER_SOURCE_TYPE_HEADER);
    if (!parse_transfer_source_type_wire_value(source_type, &metadata.source_type)) {
        throw TransferFailure(TransferRpcCode::BadRequest,
                              invalid_header_message(server_contract::TRANSFER_SOURCE_TYPE_HEADER,
                                                     "unsupported value `" + source_type + "`"));
    }
    metadata.compression = optional_header_or(request, server_contract::TRANSFER_COMPRESSION_HEADER, "none");
    require_one_of(server_contract::TRANSFER_COMPRESSION_HEADER, metadata.compression, {"none", "zstd"});
    const std::string symlink_mode =
        optional_header_or(request, server_contract::TRANSFER_SYMLINK_MODE_HEADER, "preserve");
    if (!parse_transfer_symlink_mode_wire_value(symlink_mode, &metadata.symlink_mode)) {
        throw TransferFailure(TransferRpcCode::BadRequest,
                              invalid_header_message(server_contract::TRANSFER_SYMLINK_MODE_HEADER,
                                                     "unsupported value `" + symlink_mode + "`"));
    }
    return metadata;
}

void write_transfer_export_headers(HttpResponse& response, const ExportedPayload& payload) {
    response.headers["Content-Type"] = server_contract::TRANSFER_EXPORT_CONTENT_TYPE;
    response.headers[server_contract::TRANSFER_SOURCE_TYPE_HEADER] = transfer_source_type_wire_value(payload.source_type);
    response.headers[server_contract::TRANSFER_COMPRESSION_HEADER] = "none";
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
        {"source_type", transfer_source_type_wire_value(summary.source_type)},
        {"bytes_copied", summary.bytes_copied},
        {"files_copied", summary.files_copied},
        {"directories_copied", summary.directories_copied},
        {"replaced", summary.replaced},
        {"warnings", transfer_warnings_json(summary.warnings)},
    };
}

void log_transfer_import_summary(const std::string& destination_path, const ImportSummary& summary) {
    LogMessageBuilder message("transfer/import");
    message.quoted_field("destination", destination_path)
        .field("bytes_copied", summary.bytes_copied)
        .field("files_copied", summary.files_copied)
        .field("directories_copied", summary.directories_copied)
        .bool_field("replaced", summary.replaced);
    log_message(LOG_INFO, "server", message.str());
}
