#include <sstream>
#include <stdexcept>
#include <string>

#include "common.h"
#include "logging.h"
#include "patch_engine.h"
#include "platform.h"
#include "process_session.h"
#include "server_routes.h"
#include "transfer_ops.h"

namespace {

std::string resolve_workdir(const AppState& state, const Json& body) {
    const std::string raw = body.value("workdir", state.config.default_workdir);
    if (raw.empty()) {
        return state.config.default_workdir;
    }
    if (platform::is_absolute_path(raw)) {
        return platform::normalize_path_separators(raw);
    }
    return platform::join_path(state.config.default_workdir, raw);
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

bool contains_text(const std::string& value, const std::string& needle) {
    return value.find(needle) != std::string::npos;
}

unsigned long requested_max_output_tokens(const Json& body) {
    const Json::const_iterator it = body.find("max_output_tokens");
    return it == body.end() ? DEFAULT_MAX_OUTPUT_TOKENS : it->get<unsigned long>();
}

std::string transfer_error_code(const std::string& message) {
    if (contains_text(message, "not absolute")) {
        return "transfer_path_not_absolute";
    }
    if (contains_text(message, "destination path") &&
        contains_text(message, "already exists")) {
        return "transfer_destination_exists";
    }
    if (contains_text(message, "destination parent") &&
        contains_text(message, "does not exist")) {
        return "transfer_parent_missing";
    }
    if ((contains_text(message, "destination path") &&
         contains_text(message, "is a directory")) ||
        (contains_text(message, "destination path") &&
         contains_text(message, "is not a directory")) ||
        (contains_text(message, "destination path") &&
         contains_text(message, "is not a regular file"))) {
        return "transfer_destination_unsupported";
    }
    if (contains_text(message, "transfer compression") ||
        contains_text(message, "does not support transfer compression")) {
        return "transfer_compression_unsupported";
    }
    if (contains_text(message, "unsupported symlink") ||
        contains_text(message, "unsupported entry") ||
        contains_text(message, "unsupported transfer source type") ||
        contains_text(message, "regular file or directory") ||
        contains_text(message, "archive entry is not a regular file") ||
        contains_text(message, "archive file entry cannot target root") ||
        contains_text(message, "archive path escapes") ||
        contains_text(message, "archive path must be relative") ||
        contains_text(message, "archive path contains empty component")) {
        return "transfer_source_unsupported";
    }
    if (contains_text(message, "transfer source missing") ||
        contains_text(message, "No such file or directory")) {
        return "transfer_source_missing";
    }
    return "transfer_failed";
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
            {"supports_image_read", false},
            {"supports_transfer_compression", false},
        }
    );
    return response;
}

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

        Json exec_response = state.sessions.start_command(
            body.at("cmd").get<std::string>(),
            resolve_workdir(state, body),
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

HttpResponse handle_patch_apply(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const PatchApplyResult result = apply_patch(
            resolve_workdir(state, body),
            body.at("patch").get<std::string>()
        );
        std::ostringstream summary;
        summary << "patch/apply patch_len=" << body.at("patch").get<std::string>().size();
        log_message(LOG_INFO, "server", summary.str());
        write_json(response, Json{{"output", result.output}});
    } catch (const std::exception& ex) {
        log_message(LOG_WARN, "server", std::string("patch/apply failed: ") + ex.what());
        write_rpc_error(response, 400, "patch_failed", ex.what());
    }

    return response;
}

HttpResponse handle_transfer_export(const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        if (body.value("compression", std::string("none")) != "none") {
            throw std::runtime_error("this daemon does not support transfer compression");
        }
        const ExportedPayload payload = export_path(body.at("path").get<std::string>());
        log_message(
            LOG_INFO,
            "server",
            "transfer/export path=`" + body.at("path").get<std::string>() +
                "` source_type=`" + payload.source_type + "`"
        );
        response.headers["Content-Type"] = "application/octet-stream";
        response.headers["x-remote-exec-source-type"] = payload.source_type;
        response.headers["x-remote-exec-compression"] = "none";
        response.body = payload.bytes;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        write_rpc_error(response, 400, transfer_error_code(message), message);
    }

    return response;
}

HttpResponse handle_transfer_path_info(const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const PathInfo info = path_info(body.at("path").get<std::string>());
        write_json(
            response,
            Json{
                {"exists", info.exists},
                {"is_directory", info.is_directory},
            }
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/path-info failed: " + message);
        write_rpc_error(response, 400, transfer_error_code(message), message);
    }

    return response;
}

HttpResponse handle_transfer_import(const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const std::string compression = request.header("x-remote-exec-compression");
        if (!compression.empty() && compression != "none") {
            throw std::runtime_error("this daemon does not support transfer compression");
        }
        const ImportSummary summary = import_path(
            request.body,
            request.header("x-remote-exec-source-type"),
            request.header("x-remote-exec-destination-path"),
            request.header("x-remote-exec-overwrite"),
            request.header("x-remote-exec-create-parent") == "true"
        );
        {
            std::ostringstream message;
            message << "transfer/import destination=`"
                    << request.header("x-remote-exec-destination-path")
                    << "` bytes_copied=" << summary.bytes_copied
                    << " files_copied=" << summary.files_copied
                    << " directories_copied=" << summary.directories_copied
                    << " replaced=" << (summary.replaced ? "true" : "false");
            log_message(LOG_INFO, "server", message.str());
        }
        write_json(
            response,
            Json{
                {"source_type", summary.source_type},
                {"bytes_copied", summary.bytes_copied},
                {"files_copied", summary.files_copied},
                {"directories_copied", summary.directories_copied},
                {"replaced", summary.replaced},
            }
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/import failed: " + message);
        write_rpc_error(response, 400, transfer_error_code(message), message);
    }

    return response;
}

}  // namespace

LogLevel level_for_status(int status) {
    if (status >= 500) {
        return LOG_ERROR;
    }
    if (status >= 400) {
        return LOG_WARN;
    }
    return LOG_INFO;
}

HttpResponse route_request(AppState& state, const HttpRequest& request) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        HttpResponse response;
        write_bearer_auth_challenge(response);
        return response;
    }

    if (request.method != "POST") {
        return make_rpc_error_response(405, "method_not_allowed", "only POST is supported");
    }

    if (request.path == "/v1/health") {
        return handle_health(state);
    }

    if (request.path == "/v1/target-info") {
        return handle_target_info(state);
    }

    if (request.path == "/v1/image/read") {
        return make_rpc_error_response(
            400,
            "image_unsupported",
            "image read is not supported on this target"
        );
    }

    if (request.path == "/v1/exec/start") {
        return handle_exec_start(state, request);
    }

    if (request.path == "/v1/exec/write") {
        return handle_exec_write(state, request);
    }

    if (request.path == "/v1/patch/apply") {
        return handle_patch_apply(state, request);
    }

    if (request.path == "/v1/transfer/export") {
        return handle_transfer_export(request);
    }

    if (request.path == "/v1/transfer/path-info") {
        return handle_transfer_path_info(request);
    }

    if (request.path == "/v1/transfer/import") {
        return handle_transfer_import(request);
    }

    return make_rpc_error_response(404, "not_found", "unknown endpoint");
}
