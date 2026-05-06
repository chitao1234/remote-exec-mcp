#include <string>
#include <vector>

#include "path_policy.h"
#include "server_request_utils.h"
#include "transfer_ops.h"

namespace {

const CompiledFilesystemSandbox* active_sandbox(const AppState& state) {
    return state.sandbox_enabled ? &state.sandbox : NULL;
}

std::vector<std::string> transfer_exclude_or_empty(const Json& body) {
    const Json::const_iterator it = body.find("exclude");
    if (it == body.end() || it->is_null()) {
        return std::vector<std::string>();
    }
    return it->get<std::vector<std::string> >();
}

}  // namespace

bool reject_before_route(
    const AppState& state,
    const HttpRequest& request,
    HttpResponse* response
) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        write_bearer_auth_challenge(*response);
        return true;
    }

    if (request.method != "POST") {
        write_rpc_error(*response, 405, "method_not_allowed", "only POST is supported");
        return true;
    }

    return false;
}

std::string resolve_workdir(const AppState& state, const Json& body) {
    const std::string raw = body.value("workdir", state.config.default_workdir);
    if (raw.empty()) {
        return state.config.default_workdir;
    }

    const PathPolicy policy = host_path_policy();
    if (is_absolute_for_policy(policy, raw)) {
        return normalize_for_system(policy, raw);
    }
    return join_for_policy(policy, state.config.default_workdir, raw);
}

std::string resolve_input_path(
    const AppState& state,
    const Json& body,
    const std::string& key
) {
    const std::string raw = body.at(key).get<std::string>();
    const PathPolicy policy = host_path_policy();
    if (is_absolute_for_policy(policy, raw)) {
        return normalize_for_system(policy, raw);
    }
    return join_for_policy(policy, resolve_workdir(state, body), raw);
}

void authorize_sandbox_path(
    const AppState& state,
    SandboxAccess access,
    const std::string& path
) {
    authorize_path(host_path_policy(), active_sandbox(state), access, path);
}

std::string resolve_absolute_transfer_path(const std::string& path) {
    const PathPolicy policy = host_path_policy();
    if (!is_absolute_for_policy(policy, path)) {
        throw TransferFailure(
            TransferRpcCode::PathNotAbsolute,
            "transfer path is not absolute"
        );
    }
    return normalize_for_system(policy, path);
}

TransferExportRequestSpec prepare_transfer_export_request(
    const AppState& state,
    const Json& body
) {
    require_uncompressed_transfer(body.value("compression", std::string("none")));

    TransferExportRequestSpec request;
    request.path = resolve_absolute_transfer_path(body.at("path").get<std::string>());
    authorize_sandbox_path(state, SANDBOX_READ, request.path);
    request.symlink_mode = body.value("symlink_mode", std::string("preserve"));
    request.exclude = transfer_exclude_or_empty(body);
    request.source_type = export_path_source_type(request.path, request.symlink_mode);
    return request;
}

TransferImportRequestSpec prepare_transfer_import_request(
    const AppState& state,
    const HttpRequest& request
) {
    TransferImportRequestSpec import_request;
    import_request.metadata = parse_transfer_import_metadata(request);
    require_uncompressed_transfer(import_request.metadata.compression);
    import_request.destination_path =
        resolve_absolute_transfer_path(import_request.metadata.destination_path);
    authorize_sandbox_path(state, SANDBOX_WRITE, import_request.destination_path);
    return import_request;
}

void write_transfer_error_response(HttpResponse& response, const SandboxError& ex) {
    write_rpc_error(
        response,
        400,
        transfer_error_code_name(TransferRpcCode::SandboxDenied),
        ex.what()
    );
}

void write_transfer_error_response(HttpResponse& response, const TransferFailure& failure) {
    write_rpc_error(
        response,
        transfer_error_status(failure.code),
        transfer_error_code_name(failure.code),
        failure.message
    );
}

void write_transfer_internal_error_response(
    HttpResponse& response,
    const std::string& message
) {
    write_rpc_error(
        response,
        transfer_error_status(TransferRpcCode::Internal),
        transfer_error_code_name(TransferRpcCode::Internal),
        message
    );
}
