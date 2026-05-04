#include <cstring>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/stat.h>

#include "common.h"
#include "filesystem_sandbox.h"
#include "logging.h"
#include "patch_engine.h"
#include "path_policy.h"
#include "platform.h"
#include "port_forward.h"
#include "process_session.h"
#include "rpc_failures.h"
#include "server_routes.h"
#include "transfer_ops.h"

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

TransferFailure transfer_compression_unsupported() {
    return TransferFailure(
        TransferRpcCode::CompressionUnsupported,
        "this daemon does not support transfer compression"
    );
}

ImageFailure invalid_detail_failure(const std::string& detail) {
    return ImageFailure(
        ImageRpcCode::InvalidDetail,
        "view_image.detail only supports `original`; omit `detail` for default original behavior, got `" +
            detail + "`"
    );
}

ImageFailure missing_image_failure(const std::string& path) {
    return ImageFailure(
        ImageRpcCode::Missing,
        "unable to locate image at `" + path + "`: No such file or directory"
    );
}

ImageFailure not_file_image_failure(const std::string& path) {
    return ImageFailure(
        ImageRpcCode::NotFile,
        "image path `" + path + "` is not a file"
    );
}

ImageFailure decode_failed_image(const std::string& message) {
    return ImageFailure(ImageRpcCode::DecodeFailed, message);
}

std::vector<std::string> transfer_exclude_or_empty(const Json& body) {
    const Json::const_iterator it = body.find("exclude");
    if (it == body.end() || it->is_null()) {
        return std::vector<std::string>();
    }
    return it->get<std::vector<std::string> >();
}

unsigned long requested_max_output_tokens(const Json& body) {
    const Json::const_iterator it = body.find("max_output_tokens");
    return it == body.end() ? DEFAULT_MAX_OUTPUT_TOKENS : it->get<unsigned long>();
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

std::string read_binary_file_bytes(const std::string& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    if (!input) {
        throw decode_failed_image(
            "unable to process image at `" + path + "`: unable to read file"
        );
    }
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

bool image_path_exists(const std::string& path) {
    struct stat st;
    return stat(path.c_str(), &st) == 0;
}

bool image_path_is_regular_file(const std::string& path) {
    struct stat st;
    return stat(path.c_str(), &st) == 0 && (st.st_mode & S_IFMT) == S_IFREG;
}

std::string image_mime_type(const std::string& path, const std::string& bytes) {
    if (bytes.size() >= 8 && std::memcmp(bytes.data(), "\x89PNG\r\n\x1A\n", 8) == 0) {
        return "image/png";
    }
    if (bytes.size() >= 3 &&
        static_cast<unsigned char>(bytes[0]) == 0xFF &&
        static_cast<unsigned char>(bytes[1]) == 0xD8 &&
        static_cast<unsigned char>(bytes[2]) == 0xFF) {
        return "image/jpeg";
    }
    if (bytes.size() >= 12 &&
        std::memcmp(bytes.data(), "RIFF", 4) == 0 &&
        std::memcmp(bytes.data() + 8, "WEBP", 4) == 0) {
        return "image/webp";
    }
    throw decode_failed_image(
        "unable to process image at `" + path + "`: unsupported image format"
    );
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
        }
    );
    return response;
}

HttpResponse handle_image_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const std::string detail = body.value("detail", std::string());
        if (!detail.empty() && detail != "original") {
            throw invalid_detail_failure(detail);
        }

        const std::string path = resolve_input_path(state, body, "path");
        authorize_sandbox_path(state, SANDBOX_READ, path);
        if (!image_path_exists(path)) {
            throw missing_image_failure(path);
        }
        if (!image_path_is_regular_file(path)) {
            throw not_file_image_failure(path);
        }

        const std::string bytes = read_binary_file_bytes(path);
        const std::string mime = image_mime_type(path, bytes);
        write_json(
            response,
            Json{
                {"image_url", "data:" + mime + ";base64," + base64_encode_bytes(bytes)},
                {"detail", "original"},
            }
        );
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("image/read failed: ") + ex.what());
        write_rpc_error(
            response,
            400,
            image_error_code_name(ImageRpcCode::SandboxDenied),
            ex.what()
        );
    } catch (const ImageFailure& failure) {
        log_message(LOG_WARN, "server", "image/read failed: " + failure.message);
        write_rpc_error(
            response,
            image_error_status(failure.code),
            image_error_code_name(failure.code),
            failure.message
        );
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "image/read failed: " + message);
        write_rpc_error(
            response,
            image_error_status(ImageRpcCode::Internal),
            image_error_code_name(ImageRpcCode::Internal),
            message
        );
    }

    return response;
}

HttpResponse handle_port_listen(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.listen(
                body.at("endpoint").get<std::string>(),
                body.at("protocol").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/listen failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_listen_accept(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.listen_accept(body.at("bind_id").get<std::string>())
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen/accept failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/listen/accept bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/listen/accept failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_listen_close(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.listen_close(body.at("bind_id").get<std::string>())
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/listen/close failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/listen/close bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/listen/close failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connect(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connect(
                body.at("endpoint").get<std::string>(),
                body.at("protocol").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connect failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/connect bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/connect failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connection_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connection_read(
                body.at("connection_id").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connection/read failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/connection/read bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(
            LOG_ERROR,
            "server",
            std::string("port/connection/read failed: ") + ex.what()
        );
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connection_write(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connection_write(
                body.at("connection_id").get<std::string>(),
                body.at("data").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connection/write failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/connection/write bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(
            LOG_ERROR,
            "server",
            std::string("port/connection/write failed: ") + ex.what()
        );
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_connection_close(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.connection_close(
                body.at("connection_id").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/connection/close failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(
            LOG_WARN,
            "server",
            std::string("port/connection/close bad request: ") + ex.what()
        );
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(
            LOG_ERROR,
            "server",
            std::string("port/connection/close failed: ") + ex.what()
        );
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_udp_read(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.udp_datagram_read(body.at("bind_id").get<std::string>())
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/read failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/read bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/udp/read failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_port_udp_write(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        write_json(
            response,
            state.port_forwards.udp_datagram_write(
                body.at("bind_id").get<std::string>(),
                body.at("peer").get<std::string>(),
                body.at("data").get<std::string>()
            )
        );
    } catch (const PortForwardError& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/write failed: ") + ex.what());
        write_rpc_error(response, ex.status(), ex.code(), ex.what());
    } catch (const Json::exception& ex) {
        log_message(LOG_WARN, "server", std::string("port/udp/write bad request: ") + ex.what());
        write_rpc_error(response, 400, "bad_request", ex.what());
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("port/udp/write failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

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
        const std::string workdir = resolve_workdir(state, body);
        authorize_sandbox_path(state, SANDBOX_EXEC_CWD, workdir);

        Json exec_response = state.sessions.start_command(
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

HttpResponse handle_patch_apply(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        PatchPathAuthorizer authorizer;
        if (state.sandbox_enabled) {
            authorizer = [&state](const std::string& path) {
                authorize_sandbox_path(state, SANDBOX_WRITE, path);
            };
        }
        const PatchApplyResult result = apply_patch(
            resolve_workdir(state, body),
            body.at("patch").get<std::string>(),
            authorizer
        );
        std::ostringstream summary;
        summary << "patch/apply patch_len=" << body.at("patch").get<std::string>().size();
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
        response.headers["Content-Type"] = "application/octet-stream";
        response.headers["x-remote-exec-source-type"] = payload.source_type;
        response.headers["x-remote-exec-compression"] = "none";
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
        require_uncompressed_transfer(request.header("x-remote-exec-compression"));
        const std::string destination_path =
            resolve_absolute_transfer_path(request.header("x-remote-exec-destination-path"));
        authorize_sandbox_path(state, SANDBOX_WRITE, destination_path);
        const ImportSummary summary = import_path(
            request.body,
            request.header("x-remote-exec-source-type"),
            destination_path,
            request.header("x-remote-exec-overwrite"),
            request.header("x-remote-exec-create-parent") == "true",
            transfer_symlink_mode_or_default(request.header("x-remote-exec-symlink-mode"))
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

}  // namespace

void require_uncompressed_transfer(const std::string& compression) {
    if (!compression.empty() && compression != "none") {
        throw transfer_compression_unsupported();
    }
}

std::string transfer_symlink_mode_or_default(const std::string& symlink_mode) {
    return symlink_mode.empty() ? "preserve" : symlink_mode;
}

Json transfer_warnings_json(const std::vector<TransferWarning>& warnings) {
    Json json = Json::array();
    for (std::size_t i = 0; i < warnings.size(); ++i) {
        json.push_back(Json{
            {"code", warnings[i].code},
            {"message", warnings[i].message},
        });
    }
    return json;
}

Json transfer_summary_json(const ImportSummary& summary) {
    return Json{
        {"source_type", summary.source_type},
        {"bytes_copied", summary.bytes_copied},
        {"files_copied", summary.files_copied},
        {"directories_copied", summary.directories_copied},
        {"replaced", summary.replaced},
        {"warnings", transfer_warnings_json(summary.warnings)},
    };
}

void log_transfer_import_summary(const std::string& destination_path, const ImportSummary& summary) {
    std::ostringstream message;
    message << "transfer/import destination=`" << destination_path
            << "` bytes_copied=" << summary.bytes_copied
            << " files_copied=" << summary.files_copied
            << " directories_copied=" << summary.directories_copied
            << " replaced=" << (summary.replaced ? "true" : "false");
    log_message(LOG_INFO, "server", message.str());
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
        return handle_image_read(state, request);
    }

    if (request.path == "/v1/port/listen") {
        return handle_port_listen(state, request);
    }

    if (request.path == "/v1/port/listen/accept") {
        return handle_port_listen_accept(state, request);
    }

    if (request.path == "/v1/port/listen/close") {
        return handle_port_listen_close(state, request);
    }

    if (request.path == "/v1/port/connect") {
        return handle_port_connect(state, request);
    }

    if (request.path == "/v1/port/connection/read") {
        return handle_port_connection_read(state, request);
    }

    if (request.path == "/v1/port/connection/write") {
        return handle_port_connection_write(state, request);
    }

    if (request.path == "/v1/port/connection/close") {
        return handle_port_connection_close(state, request);
    }

    if (request.path == "/v1/port/udp/read") {
        return handle_port_udp_read(state, request);
    }

    if (request.path == "/v1/port/udp/write") {
        return handle_port_udp_write(state, request);
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
        return handle_transfer_export(state, request);
    }

    if (request.path == "/v1/transfer/path-info") {
        return handle_transfer_path_info(state, request);
    }

    if (request.path == "/v1/transfer/import") {
        return handle_transfer_import(state, request);
    }

    return make_rpc_error_response(404, "not_found", "unknown endpoint");
}
