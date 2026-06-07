#include "rpc/transfer_request_utils.h"

#include "policy/path_policy.h"
#include "rpc/server_request_utils.h"

namespace {

std::vector<std::string> transfer_exclude_or_empty(const Json& body) {
    const Json::const_iterator it = body.find("exclude");
    if (it == body.end() || it->is_null()) {
        return std::vector<std::string>();
    }
    return it->get<std::vector<std::string>>();
}

TransferPathAuthorizer make_transfer_write_authorizer(const PathResolutionContext& context) {
    if (context.active_sandbox == nullptr) {
        return TransferPathAuthorizer();
    }
    return [context](const std::string& path) { authorize_sandbox_path(context, SANDBOX_WRITE, path); };
}

} // namespace

std::string resolve_absolute_transfer_path(const std::string& path) {
    const PathPolicy policy = host_path_policy();
    if (!is_absolute_for_policy(policy, path)) {
        throw TransferFailure(TransferRpcCode::PathNotAbsolute, "transfer path is not absolute");
    }
    return normalize_for_system(policy, path);
}

std::string
resolve_authorized_transfer_path(const PathResolutionContext& context, const std::string& path, SandboxAccess access) {
    const std::string resolved = resolve_absolute_transfer_path(path);
    authorize_sandbox_path(context, access, resolved);
    return resolved;
}

TransferPathAuthorizer make_transfer_read_authorizer(const PathResolutionContext& context) {
    if (context.active_sandbox == nullptr) {
        return TransferPathAuthorizer();
    }
    return [context](const std::string& path) { authorize_sandbox_path(context, SANDBOX_READ, path); };
}

TransferExportRequestSpec prepare_transfer_export_request(const TransferRouteContext& context, const Json& body) {
    require_uncompressed_transfer(body.value("compression", std::string("none")));

    TransferExportRequestSpec request;
    request.path = resolve_authorized_transfer_path(context.paths, body.at("path").get<std::string>(), SANDBOX_READ);
    const std::string symlink_mode = body.value("symlink_mode", std::string("preserve"));
    if (!parse_transfer_symlink_mode_wire_value(symlink_mode, &request.symlink_mode)) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "unsupported transfer symlink mode");
    }
    request.exclude = transfer_exclude_or_empty(body);
    request.authorizer = make_transfer_read_authorizer(context.paths);
    request.source_type = export_path_source_type(request.path, request.symlink_mode);
    return request;
}

TransferImportRequestSpec prepare_transfer_import_request(const TransferRouteContext& context,
                                                          const HttpRequest& request) {
    TransferImportRequestSpec import_request;
    import_request.metadata = parse_transfer_import_metadata(request);
    require_uncompressed_transfer(import_request.metadata.compression);
    import_request.destination_path =
        resolve_authorized_transfer_path(context.paths, import_request.metadata.destination_path, SANDBOX_WRITE);
    import_request.limits = context.limits;
    import_request.authorizer = make_transfer_write_authorizer(context.paths);
    return import_request;
}

void write_transfer_error_response(HttpResponse& response, const SandboxError& ex) {
    write_rpc_error(response, 400, transfer_error_code_name(TransferRpcCode::SandboxDenied), ex.what());
}

void write_transfer_error_response(HttpResponse& response, const TransferFailure& failure) {
    write_rpc_error(
        response, transfer_error_status(failure.code), transfer_error_code_name(failure.code), failure.message);
}

void write_transfer_internal_error_response(HttpResponse& response, const std::string& message) {
    write_rpc_error(response,
                    transfer_error_status(TransferRpcCode::Internal),
                    transfer_error_code_name(TransferRpcCode::Internal),
                    message);
}
