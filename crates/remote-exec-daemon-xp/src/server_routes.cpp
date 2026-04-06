#include <algorithm>
#include <cctype>
#include <sstream>
#include <stdexcept>
#include <string>

#include "common.h"
#include "logging.h"
#include "patch_engine.h"
#include "server_routes.h"
#include "transfer_ops.h"

namespace {

bool is_absolute_windows_path(const std::string& path) {
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 &&
            path[1] == ':' && (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
}

std::string normalize_windows_separators(std::string path) {
    std::replace(path.begin(), path.end(), '/', '\\');
    return path;
}

std::string join_windows_path(const std::string& base, const std::string& relative) {
    if (base.empty()) {
        return normalize_windows_separators(relative);
    }
    std::string joined = normalize_windows_separators(base);
    if (!joined.empty() && joined[joined.size() - 1] != '\\') {
        joined += '\\';
    }
    joined += normalize_windows_separators(relative);
    return joined;
}

std::string resolve_workdir(const AppState& state, const Json& body) {
    const std::string raw = body.value("workdir", state.config.default_workdir);
    if (raw.empty()) {
        return state.config.default_workdir;
    }
    if (is_absolute_windows_path(raw)) {
        return normalize_windows_separators(raw);
    }
    return join_windows_path(state.config.default_workdir, raw);
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

HttpResponse handle_health(const AppState& state) {
    HttpResponse response;
    write_json(
        response,
        Json {
            {"status", "ok"},
            {"daemon_version", REMOTE_EXEC_XP_VERSION},
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
            {"daemon_version", REMOTE_EXEC_XP_VERSION},
            {"daemon_instance_id", state.daemon_instance_id},
            {"hostname", state.hostname},
            {"platform", "windows"},
            {"arch", "x86"},
            {"supports_pty", false},
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
        if (body.value("tty", false)) {
            return make_rpc_error_response(
                400,
                "tty_unsupported",
                "tty is not supported on this host"
            );
        }

        const std::string shell = body.value("shell", "");
        if (!shell.empty() && shell != "cmd" && shell != "cmd.exe") {
            return make_rpc_error_response(
                400,
                "unsupported_shell",
                "only cmd.exe is supported on this target"
            );
        }

        Json exec_response = state.sessions.start_command(
            body.at("cmd").get<std::string>(),
            resolve_workdir(state, body),
            shell,
            body.value("yield_time_ms", 0UL),
            body.value("max_output_tokens", 0UL)
        );
        log_message(
            LOG_INFO,
            "server",
            "exec/start target=`" + state.config.target + "` cmd_preview=`" +
                preview_text(body.at("cmd").get<std::string>(), 120) + "`"
        );
        exec_response["daemon_instance_id"] = state.daemon_instance_id;
        write_json(response, exec_response);
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
            body.value("yield_time_ms", 0UL),
            body.value("max_output_tokens", 0UL)
        );
        exec_response["daemon_instance_id"] = state.daemon_instance_id;
        write_json(response, exec_response);
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        if (message == "unknown_session") {
            log_message(LOG_WARN, "server", "exec/write failed: unknown_session");
            write_rpc_error(response, 400, "unknown_session", "Unknown daemon session");
        } else {
            log_message(LOG_ERROR, "server", std::string("exec/write failed: ") + message);
            write_rpc_error(response, 500, "internal_error", message);
        }
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
        log_message(LOG_WARN, "server", std::string("transfer/export failed: ") + ex.what());
        write_rpc_error(response, 400, "transfer_failed", ex.what());
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
            request.header("x-remote-exec-overwrite") == "replace",
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
        log_message(LOG_WARN, "server", std::string("transfer/import failed: ") + ex.what());
        write_rpc_error(response, 400, "transfer_failed", ex.what());
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

    if (request.path == "/v1/transfer/import") {
        return handle_transfer_import(request);
    }

    return make_rpc_error_response(404, "not_found", "unknown endpoint");
}
