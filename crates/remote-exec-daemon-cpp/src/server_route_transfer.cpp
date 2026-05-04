#include <string>
#include <vector>

#include "filesystem_sandbox.h"
#include "logging.h"
#include "path_policy.h"
#include "rpc_failures.h"
#include "server_route_transfer.h"
#include "transfer_http_codec.h"

namespace {

const CompiledFilesystemSandbox* active_sandbox(const AppState& state) {
    return state.sandbox_enabled ? &state.sandbox : NULL;
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

std::vector<std::string> transfer_exclude_or_empty(const Json& body) {
    const Json::const_iterator it = body.find("exclude");
    if (it == body.end() || it->is_null()) {
        return std::vector<std::string>();
    }
    return it->get<std::vector<std::string> >();
}

}  // namespace

HttpResponse handle_transfer_export(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        require_uncompressed_transfer(body.value("compression", std::string("none")));
        const std::string path = resolve_absolute_transfer_path(body.at("path").get<std::string>());
        authorize_sandbox_path(state, SANDBOX_READ, path);
        const std::vector<std::string> exclude = transfer_exclude_or_empty(body);
        const ExportedPayload payload = export_path(
            path,
            body.value("symlink_mode", std::string("preserve")),
            exclude
        );
        log_message(
            LOG_INFO,
            "server",
            "transfer/export path=`" + path + "` source_type=`" + payload.source_type + "`"
        );
        write_transfer_export_headers(response, payload);
        response.body = payload.bytes;
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("transfer/export failed: ") + ex.what());
        write_rpc_error(
            response,
            400,
            transfer_error_code_name(TransferRpcCode::SandboxDenied),
            ex.what()
        );
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
        write_rpc_error(
            response,
            transfer_error_status(failure.code),
            transfer_error_code_name(failure.code),
            failure.message
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        write_rpc_error(
            response,
            transfer_error_status(TransferRpcCode::Internal),
            transfer_error_code_name(TransferRpcCode::Internal),
            message
        );
    }

    return response;
}

HttpResponse handle_transfer_path_info(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const std::string path = resolve_absolute_transfer_path(body.at("path").get<std::string>());
        authorize_sandbox_path(state, SANDBOX_WRITE, path);
        const PathInfo info = path_info(path);
        write_json(
            response,
            Json{
                {"exists", info.exists},
                {"is_directory", info.is_directory},
            }
        );
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("transfer/path-info failed: ") + ex.what());
        write_rpc_error(
            response,
            400,
            transfer_error_code_name(TransferRpcCode::SandboxDenied),
            ex.what()
        );
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/path-info failed: " + failure.message);
        write_rpc_error(
            response,
            transfer_error_status(failure.code),
            transfer_error_code_name(failure.code),
            failure.message
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/path-info failed: " + message);
        write_rpc_error(
            response,
            transfer_error_status(TransferRpcCode::Internal),
            transfer_error_code_name(TransferRpcCode::Internal),
            message
        );
    }

    return response;
}

HttpResponse handle_transfer_import(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const TransferImportMetadata metadata = parse_transfer_import_metadata(request);
        require_uncompressed_transfer(metadata.compression);
        const std::string destination_path =
            resolve_absolute_transfer_path(metadata.destination_path);
        authorize_sandbox_path(state, SANDBOX_WRITE, destination_path);
        const ImportSummary summary = import_path(
            request.body,
            metadata.source_type,
            destination_path,
            metadata.overwrite,
            metadata.create_parent,
            metadata.symlink_mode
        );
        log_transfer_import_summary(destination_path, summary);
        write_json(response, transfer_summary_json(summary));
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("transfer/import failed: ") + ex.what());
        write_rpc_error(
            response,
            400,
            transfer_error_code_name(TransferRpcCode::SandboxDenied),
            ex.what()
        );
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/import failed: " + failure.message);
        write_rpc_error(
            response,
            transfer_error_status(failure.code),
            transfer_error_code_name(failure.code),
            failure.message
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/import failed: " + message);
        write_rpc_error(
            response,
            transfer_error_status(TransferRpcCode::Internal),
            transfer_error_code_name(TransferRpcCode::Internal),
            message
        );
    }

    return response;
}
