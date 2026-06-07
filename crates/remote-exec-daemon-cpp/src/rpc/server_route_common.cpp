#include <string>

#include "core/common.h"
#include "exec/process_session.h"
#include "exec/session_store.h"
#include "patch/patch_engine.h"
#include "platform/platform.h"
#include "rpc/capabilities_http_codec.h"
#include "rpc/exec_request_utils.h"
#include "rpc/rpc_failures.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/transfer_request_utils.h"

namespace {

HttpResponse run_rpc_route(const RpcRouteBody& body) {
    HttpResponse response;
    response.status = 200;
    body(response);
    return response;
}

} // namespace

HttpResponse handle_transfer_rpc_route(const std::string& route_name, const RpcRouteBody& body) {
    try {
        return run_rpc_route(body);
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", route_name + " failed: " + ex.what());
        HttpResponse response;
        response.status = 400;
        write_transfer_error_response(response, ex);
        return response;
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", route_name + " failed: " + failure.message);
        HttpResponse response;
        response.status = transfer_error_status(failure.code);
        write_transfer_error_response(response, failure);
        return response;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", route_name + " failed: " + message);
        HttpResponse response;
        response.status = transfer_error_status(TransferRpcCode::Internal);
        write_transfer_internal_error_response(response, message);
        return response;
    }
}

HttpResponse handle_image_rpc_route(const std::string& route_name, const RpcRouteBody& body) {
    try {
        return run_rpc_route(body);
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", route_name + " failed: " + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(
            response,
            400,
            image_error_code_name(ImageRpcCode::SandboxDenied),
            ex.what()
        );
        return response;
    } catch (const ImageFailure& failure) {
        log_message(LOG_WARN, "server", route_name + " failed: " + failure.message);
        HttpResponse response;
        response.status = image_error_status(failure.code);
        write_rpc_error(
            response,
            image_error_status(failure.code),
            image_error_code_name(failure.code),
            failure.message
        );
        return response;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", route_name + " failed: " + message);
        HttpResponse response;
        response.status = image_error_status(ImageRpcCode::Internal);
        write_rpc_error(
            response,
            image_error_status(ImageRpcCode::Internal),
            image_error_code_name(ImageRpcCode::Internal),
            message
        );
        return response;
    }
}

HttpResponse handle_exec_rpc_route(
    const std::string& route_name,
    ExecRouteKind kind,
    const RpcRouteBody& body
) {
    try {
        return run_rpc_route(body);
    } catch (const SessionLimitError& ex) {
        log_message(LOG_WARN, "server", route_name + " rejected: " + ex.what());
        HttpResponse response;
        response.status = 429;
        write_rpc_error(response, 429, "session_limit_exceeded", ex.what());
        return response;
    } catch (const SandboxError& ex) {
        const std::string action = kind == ExecRouteKind::Start ? " denied: " : " rejected: ";
        log_message(LOG_WARN, "server", route_name + action + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "sandbox_denied", ex.what());
        return response;
    } catch (const UnknownSessionError& ex) {
        log_message(LOG_WARN, "server", route_name + " unknown session: " + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "unknown_session", ex.what());
        return response;
    } catch (const StdinClosedError& ex) {
        log_message(LOG_WARN, "server", route_name + " stdin closed: " + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "stdin_closed", ex.what());
        return response;
    } catch (const ProcessPtyResizeUnsupportedError& ex) {
        log_message(LOG_WARN, "server", route_name + " pty resize unsupported: " + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "tty_unsupported", ex.what());
        return response;
    } catch (const ExecRequestFailure& ex) {
        log_message(level_for_status(ex.status), "server", route_name + " rejected: " + ex.what());
        HttpResponse response;
        response.status = ex.status;
        write_rpc_error(response, ex.status, ex.code, ex.message);
        return response;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_ERROR, "server", route_name + " failed: " + message);
        HttpResponse response;
        response.status = 500;
        write_rpc_error(response, 500, "internal_error", message);
        return response;
    }
}

HttpResponse handle_patch_rpc_route(const RpcRouteBody& body) {
    try {
        return run_rpc_route(body);
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("patch/apply denied: ") + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "sandbox_denied", ex.what());
        return response;
    } catch (const std::exception& ex) {
        log_message(LOG_WARN, "server", std::string("patch/apply failed: ") + ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "patch_failed", ex.what());
        return response;
    }
}

HttpResponse make_rpc_error_response(
    int status,
    const std::string& code,
    const std::string& message
) {
    HttpResponse response;
    response.status = status;
    write_rpc_error(response, status, code, message);
    return response;
}

HttpResponse handle_health(const HealthRouteContext& context) {
    HttpResponse response;
    write_json(
        response,
        Json{
            {"status", "ok"},
            {"daemon_version", REMOTE_EXEC_CPP_VERSION},
            {"daemon_instance_id", context.daemon_instance_id},
        }
    );
    return response;
}

HttpResponse handle_target_info(const TargetInfoRouteContext& context) {
    HttpResponse response;
    Json body{
        {"target", context.target},
        {"daemon_version", REMOTE_EXEC_CPP_VERSION},
        {"daemon_instance_id", context.daemon_instance_id},
        {"hostname", context.hostname},
        {"platform", platform::platform_name()},
        {"arch", platform::arch_name()},
    };
    write_daemon_capabilities(&body, context.capabilities);
    write_json(response, body);
    return response;
}

HttpResponse handle_patch_apply(const PatchRouteContext& context, const HttpRequest& request) {
    return handle_patch_rpc_route([&](HttpResponse& response) {
        const Json body = parse_json_body(request);
        const std::string workdir = resolve_workdir(context.paths, body);
        const std::string patch_text = body.at("patch").get<std::string>();
        const PatchApplyResult result =
            apply_patch(workdir, patch_text, make_patch_path_authorizer(context.paths));
        LogMessageBuilder summary("patch/apply");
        summary.field("patch_len", patch_text.size());
        log_message(LOG_INFO, "server", summary.str());
        write_json(
            response,
            Json{
                {"output", result.output},
                {"daemon_instance_id", context.daemon_instance_id},
                {"updated_paths", result.updated_paths},
            }
        );
    });
}

LogLevel level_for_status(int status) {
    if (status >= 500) {
        return LOG_ERROR;
    }
    if (status >= 400) {
        return LOG_WARN;
    }
    return LOG_INFO;
}
