#include <sstream>
#include <string>

#include "common.h"
#include "patch_engine.h"
#include "platform.h"
#include "process_session.h"
#include "server_route_common.h"
#include "server_request_utils.h"

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

HttpResponse handle_health(const AppState& state) {
    HttpResponse response;
    write_json(
        response,
        Json {
            {"status", "ok"},
            {"daemon_version", REMOTE_EXEC_CPP_VERSION},
            {"daemon_instance_id", state.daemon_instance_id},
        }
    );
    return response;
}

HttpResponse handle_target_info(const AppState& state) {
    HttpResponse response;
    write_json(
        response,
        Json {
            {"target", state.config.target},
            {"daemon_version", REMOTE_EXEC_CPP_VERSION},
            {"daemon_instance_id", state.daemon_instance_id},
            {"hostname", state.hostname},
            {"platform", platform::platform_name()},
            {"arch", platform::arch_name()},
            {"supports_pty", process_session_supports_pty()},
            {"supports_image_read", true},
            {"supports_transfer_compression", false},
            {"supports_port_forward", true},
            {"port_forward_protocol_version", 2},
        }
    );
    return response;
}

HttpResponse handle_patch_apply(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const std::string workdir = resolve_workdir(state, body);
        const std::string patch_text = body.at("patch").get<std::string>();
        const PatchApplyResult result = apply_patch(
            workdir,
            patch_text,
            make_patch_path_authorizer(state)
        );
        std::ostringstream summary;
        summary << "patch/apply patch_len=" << patch_text.size();
        log_message(LOG_INFO, "server", summary.str());
        write_json(response, Json{{"output", result.output}});
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("patch/apply denied: ") + ex.what());
        write_rpc_error(response, 400, "sandbox_denied", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_WARN, "server", std::string("patch/apply failed: ") + ex.what());
        write_rpc_error(response, 400, "patch_failed", ex.what());
    }

    return response;
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
