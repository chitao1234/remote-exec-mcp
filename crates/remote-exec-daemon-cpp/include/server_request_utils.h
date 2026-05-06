#pragma once

#include <string>
#include <vector>

#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "rpc_failures.h"
#include "server.h"
#include "transfer_http_codec.h"

struct TransferExportRequestSpec {
    std::string path;
    std::string source_type;
    std::string symlink_mode;
    std::vector<std::string> exclude;
};

struct TransferImportRequestSpec {
    TransferImportMetadata metadata;
    std::string destination_path;
};

bool reject_before_route(
    const AppState& state,
    const HttpRequest& request,
    HttpResponse* response
);
std::string resolve_workdir(const AppState& state, const Json& body);
std::string resolve_input_path(
    const AppState& state,
    const Json& body,
    const std::string& key
);
void authorize_sandbox_path(
    const AppState& state,
    SandboxAccess access,
    const std::string& path
);
std::string resolve_absolute_transfer_path(const std::string& path);
TransferExportRequestSpec prepare_transfer_export_request(
    const AppState& state,
    const Json& body
);
TransferImportRequestSpec prepare_transfer_import_request(
    const AppState& state,
    const HttpRequest& request
);
void write_transfer_error_response(HttpResponse& response, const SandboxError& ex);
void write_transfer_error_response(HttpResponse& response, const TransferFailure& failure);
void write_transfer_internal_error_response(
    HttpResponse& response,
    const std::string& message
);
