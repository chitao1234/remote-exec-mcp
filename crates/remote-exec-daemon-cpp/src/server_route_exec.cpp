#include <sstream>
#include <string>

#include "filesystem_sandbox.h"
#include "logging.h"
#include "path_policy.h"
#include "platform.h"
#include "process_session.h"
#include "server_route_common.h"
#include "server_route_exec.h"

namespace {

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

unsigned long requested_max_output_tokens(const Json& body) {
    const Json::const_iterator it = body.find("max_output_tokens");
    return it == body.end() ? DEFAULT_MAX_OUTPUT_TOKENS : it->get<unsigned long>();
}

}  // namespace

HttpResponse handle_exec_start(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const Json::const_iterator yield_time_it = body.find("yield_time_ms");
        const bool has_yield_time_ms = yield_time_it != body.end();
        const unsigned long yield_time_ms =
            has_yield_time_ms ? yield_time_it->get<unsigned long>() : 0UL;
        const bool tty_requested = body.value("tty", false);
        if (tty_requested && !process_session_supports_pty()) {
            return make_rpc_error_response(
                400,
                "tty_unsupported",
                "tty is not supported on this host"
            );
        }

        const bool login_requested = body.value("login", state.config.allow_login_shell);
        if (login_requested && !state.config.allow_login_shell) {
            return make_rpc_error_response(
                400,
                "login_shell_disabled",
                "login shells are disabled by daemon config"
            );
        }
        const std::string shell_override = body.value("shell", std::string());
        if (!shell_override.empty() && !platform::shell_supported(shell_override)) {
            return make_rpc_error_response(
                400,
                "unsupported_shell",
                "requested shell is not supported on this target"
            );
        }
        const std::string shell =
            platform::selected_shell(shell_override, state.default_shell);
        const std::string workdir = resolve_workdir(state, body);
        authorize_sandbox_path(state, SANDBOX_EXEC_CWD, workdir);

        Json exec_response = state.sessions.start_command(
            state.config.target,
            body.at("cmd").get<std::string>(),
            workdir,
            shell,
            login_requested,
            tty_requested,
            has_yield_time_ms,
            yield_time_ms,
            requested_max_output_tokens(body),
            state.config.yield_time,
            state.config.max_open_sessions
        );
        log_message(
            LOG_INFO,
            "server",
            "exec/start target=`" + state.config.target + "` cmd_preview=`" +
                preview_text(body.at("cmd").get<std::string>(), 120) + "`"
        );
        exec_response["daemon_instance_id"] = state.daemon_instance_id;
        write_json(response, exec_response);
    } catch (const SessionLimitError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/start rejected: ") + ex.what());
        write_rpc_error(response, 429, "session_limit_exceeded", ex.what());
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/start denied: ") + ex.what());
        write_rpc_error(response, 400, "sandbox_denied", ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("exec/start bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("exec/start failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_exec_write(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const Json::const_iterator yield_time_it = body.find("yield_time_ms");
        const bool has_yield_time_ms = yield_time_it != body.end();
        const unsigned long yield_time_ms =
            has_yield_time_ms ? yield_time_it->get<unsigned long>() : 0UL;
        {
            std::ostringstream message;
            message << "exec/write daemon_session_id=`"
                    << body.at("daemon_session_id").get<std::string>()
                    << "` chars_len=" << body.value("chars", std::string()).size();
            log_message(LOG_INFO, "server", message.str());
        }
        Json exec_response = state.sessions.write_stdin(
            body.at("daemon_session_id").get<std::string>(),
            body.value("chars", std::string()),
            has_yield_time_ms,
            yield_time_ms,
            requested_max_output_tokens(body),
            state.config.yield_time
        );
        exec_response["daemon_instance_id"] = state.daemon_instance_id;
        write_json(response, exec_response);
    } catch (const UnknownSessionError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write unknown session: ") + ex.what());
        write_rpc_error(response, 400, "unknown_session", ex.what());
    } catch (const StdinClosedError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write stdin closed: ") + ex.what());
        write_rpc_error(response, 400, "stdin_closed", ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_ERROR, "server", std::string("exec/write failed: ") + message);
        write_rpc_error(response, 500, "internal_error", message);
    }

    return response;
}
