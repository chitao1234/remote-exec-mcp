#pragma once

#include <string>
#include <vector>

#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "patch_engine.h"
#include "rpc_failures.h"
#include "server.h"
#include "transfer_http_codec.h"

struct TransferExportRequestSpec {
    std::string path;
    TransferSourceType source_type;
    TransferSymlinkMode symlink_mode;
    std::vector<std::string> exclude;
};

struct TransferImportRequestSpec {
    TransferImportMetadata metadata;
    std::string destination_path;
    TransferLimitConfig limits;
};

bool reject_before_route(
    const AppState& state,
    const HttpRequest& request,
    HttpResponse* response
);
std::string resolve_workdir(const AppState& state, const Json& body);
std::string resolve_authorized_workdir(
    const AppState& state,
    const Json& body,
    SandboxAccess access
);
std::string resolve_input_path(
    const AppState& state,
    const Json& body,
    const std::string& key
);
std::string resolve_authorized_input_path(
    const AppState& state,
    const Json& body,
    const std::string& key,
    SandboxAccess access
);
void authorize_sandbox_path(
    const AppState& state,
    SandboxAccess access,
    const std::string& path
);
std::string resolve_absolute_transfer_path(const std::string& path);
std::string resolve_authorized_transfer_path(
    const AppState& state,
    const std::string& path,
    SandboxAccess access
);
PatchPathAuthorizer make_patch_path_authorizer(const AppState& state);
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
