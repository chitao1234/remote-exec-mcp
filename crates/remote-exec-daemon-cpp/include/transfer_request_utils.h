#pragma once

#include <string>
#include <vector>

#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "rpc_failures.h"
#include "transfer_http_codec.h"

struct AppState;

struct TransferExportRequestSpec {
    std::string path;
    TransferSourceType source_type;
    TransferSymlinkMode symlink_mode;
    std::vector<std::string> exclude;
    TransferPathAuthorizer authorizer;
};

struct TransferImportRequestSpec {
    TransferImportMetadata metadata;
    std::string destination_path;
    TransferLimitConfig limits;
    TransferPathAuthorizer authorizer;
};

std::string resolve_absolute_transfer_path(const std::string& path);
std::string resolve_authorized_transfer_path(const AppState& state, const std::string& path, SandboxAccess access);
TransferPathAuthorizer make_transfer_read_authorizer(const AppState& state);
TransferExportRequestSpec prepare_transfer_export_request(const AppState& state, const Json& body);
TransferImportRequestSpec prepare_transfer_import_request(const AppState& state, const HttpRequest& request);
void write_transfer_error_response(HttpResponse& response, const SandboxError& ex);
void write_transfer_error_response(HttpResponse& response, const TransferFailure& failure);
void write_transfer_internal_error_response(HttpResponse& response, const std::string& message);
