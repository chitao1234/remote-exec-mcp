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
    TransferPathAuthorizer authorizer;
};

class ExecRequestFailure : public std::runtime_error {
public:
    ExecRequestFailure(int status, const std::string& code, const std::string& message);

    int status;
    std::string code;
    std::string message;
};

struct ExecPtySizeSpec {
    bool present;
    unsigned short rows;
    unsigned short cols;
};

struct ExecStartRequestSpec {
    std::string cmd;
    std::string workdir;
    std::string shell;
    bool login_requested;
    bool tty_requested;
    bool has_yield_time_ms;
    unsigned long yield_time_ms;
    unsigned long max_output_tokens;
};

struct ExecWriteRequestSpec {
    std::string daemon_session_id;
    std::string chars;
    bool has_yield_time_ms;
    unsigned long yield_time_ms;
    unsigned long max_output_tokens;
    ExecPtySizeSpec pty_size;
};

bool reject_before_route(const AppState& state, const HttpRequest& request, HttpResponse* response);
ExecStartRequestSpec prepare_exec_start_request(const AppState& state, const HttpRequest& request);
ExecWriteRequestSpec prepare_exec_write_request(const HttpRequest& request);
std::string resolve_workdir(const AppState& state, const Json& body);
std::string resolve_authorized_workdir(const AppState& state, const Json& body, SandboxAccess access);
std::string resolve_input_path(const AppState& state, const Json& body, const std::string& key);
std::string
resolve_authorized_input_path(const AppState& state, const Json& body, const std::string& key, SandboxAccess access);
void authorize_sandbox_path(const AppState& state, SandboxAccess access, const std::string& path);
std::string resolve_absolute_transfer_path(const std::string& path);
std::string resolve_authorized_transfer_path(const AppState& state, const std::string& path, SandboxAccess access);
PatchPathAuthorizer make_patch_path_authorizer(const AppState& state);
TransferExportRequestSpec prepare_transfer_export_request(const AppState& state, const Json& body);
TransferImportRequestSpec prepare_transfer_import_request(const AppState& state, const HttpRequest& request);
void write_transfer_error_response(HttpResponse& response, const SandboxError& ex);
void write_transfer_error_response(HttpResponse& response, const TransferFailure& failure);
void write_transfer_internal_error_response(HttpResponse& response, const std::string& message);
