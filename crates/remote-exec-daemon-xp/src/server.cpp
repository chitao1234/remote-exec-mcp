#include <algorithm>
#include <cstdlib>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

#include <winsock2.h>
#include <windows.h>
#include <ws2tcpip.h>

#include "common.h"
#include "http_helpers.h"
#include "logging.h"
#include "patch_engine.h"
#include "server.h"
#include "transfer_ops.h"

static std::string daemon_instance_id() {
    std::ostringstream out;
    out << GetTickCount();
    return out.str();
}

static std::string hostname_string() {
    char buffer[MAX_COMPUTERNAME_LENGTH + 1];
    DWORD size = MAX_COMPUTERNAME_LENGTH + 1;
    if (GetComputerNameA(buffer, &size) == 0) {
        return "windows-xp";
    }
    return std::string(buffer, size);
}

static std::string trim(const std::string& raw) {
    const std::string whitespace = " \t\r\n";
    const std::size_t start = raw.find_first_not_of(whitespace);
    if (start == std::string::npos) {
        return "";
    }
    const std::size_t end = raw.find_last_not_of(whitespace);
    return raw.substr(start, end - start + 1);
}

static std::string lowercase(std::string value) {
    std::transform(
        value.begin(),
        value.end(),
        value.begin(),
        [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); }
    );
    return value;
}

static HttpRequest parse_http_request(const std::string& raw) {
    const std::size_t header_end = raw.find("\r\n\r\n");
    if (header_end == std::string::npos) {
        throw std::runtime_error("invalid http request");
    }

    std::istringstream lines(raw.substr(0, header_end));
    std::string request_line;
    if (!std::getline(lines, request_line)) {
        throw std::runtime_error("missing request line");
    }
    if (!request_line.empty() && request_line[request_line.size() - 1] == '\r') {
        request_line.erase(request_line.size() - 1);
    }

    std::istringstream request_line_stream(request_line);
    HttpRequest request;
    request_line_stream >> request.method >> request.path;
    if (request.method.empty() || request.path.empty()) {
        throw std::runtime_error("invalid request line");
    }

    std::string header_line;
    while (std::getline(lines, header_line)) {
        if (!header_line.empty() && header_line[header_line.size() - 1] == '\r') {
            header_line.erase(header_line.size() - 1);
        }
        if (header_line.empty()) {
            continue;
        }

        const std::size_t colon = header_line.find(':');
        if (colon == std::string::npos) {
            continue;
        }

        request.headers[lowercase(trim(header_line.substr(0, colon)))] =
            trim(header_line.substr(colon + 1));
    }

    request.body = raw.substr(header_end + 4);
    return request;
}

static std::string read_request(SOCKET client) {
    std::string data;
    char buffer[4096];
    std::size_t expected_size = 0;
    bool parsed_headers = false;

    for (;;) {
        const int received = recv(client, buffer, sizeof(buffer), 0);
        if (received <= 0) {
            break;
        }

        data.append(buffer, received);

        if (!parsed_headers) {
            const std::size_t header_end = data.find("\r\n\r\n");
            if (header_end != std::string::npos) {
                parsed_headers = true;
                const HttpRequest request = parse_http_request(data);
                expected_size = header_end + 4;
                const std::string content_length = request.header("content-length");
                if (!content_length.empty()) {
                    expected_size += static_cast<std::size_t>(std::atoi(content_length.c_str()));
                }
            }
        }

        if (parsed_headers && data.size() >= expected_size) {
            break;
        }
    }

    return data;
}

static void send_all(SOCKET client, const std::string& data) {
    std::size_t offset = 0;
    while (offset < data.size()) {
        const int sent = send(
            client,
            data.data() + offset,
            static_cast<int>(data.size() - offset),
            0
        );
        if (sent <= 0) {
            throw std::runtime_error("send failed");
        }
        offset += static_cast<std::size_t>(sent);
    }
}

static bool is_absolute_windows_path(const std::string& path) {
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 &&
            path[1] == ':' && (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
}

static std::string normalize_windows_separators(std::string path) {
    std::replace(path.begin(), path.end(), '/', '\\');
    return path;
}

static std::string join_windows_path(const std::string& base, const std::string& relative) {
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

static std::string resolve_workdir(const AppState& state, const Json& body) {
    const std::string raw = body.value("workdir", state.config.default_workdir);
    if (raw.empty()) {
        return state.config.default_workdir;
    }
    if (is_absolute_windows_path(raw)) {
        return normalize_windows_separators(raw);
    }
    return join_windows_path(state.config.default_workdir, raw);
}

static LogLevel level_for_status(int status) {
    if (status >= 500) {
        return LOG_ERROR;
    }
    if (status >= 400) {
        return LOG_WARN;
    }
    return LOG_INFO;
}

static HttpResponse route_request(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    if (request.method != "POST") {
        write_rpc_error(response, 405, "method_not_allowed", "only POST is supported");
        return response;
    }

    if (request.path == "/v1/health") {
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

    if (request.path == "/v1/target-info") {
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
            }
        );
        return response;
    }

    if (request.path == "/v1/image/read") {
        write_rpc_error(
            response,
            400,
            "image_unsupported",
            "image read is not supported on this target"
        );
        return response;
    }

    if (request.path == "/v1/exec/start") {
        try {
            const Json body = parse_json_body(request);
            if (body.value("tty", false)) {
                write_rpc_error(response, 400, "tty_unsupported", "tty is not supported on this host");
                return response;
            }

            const std::string shell = body.value("shell", "");
            if (!shell.empty() && shell != "cmd" && shell != "cmd.exe") {
                write_rpc_error(response, 400, "unsupported_shell", "only cmd.exe is supported on this target");
                return response;
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
            write_rpc_error(response, 500, "internal_error", ex.what());
        }
        return response;
    }

    if (request.path == "/v1/exec/write") {
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
                write_rpc_error(response, 400, "unknown_session", "Unknown daemon session");
            } else {
                write_rpc_error(response, 500, "internal_error", message);
            }
        }
        return response;
    }

    if (request.path == "/v1/patch/apply") {
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
            write_rpc_error(response, 400, "patch_failed", ex.what());
        }
        return response;
    }

    if (request.path == "/v1/transfer/export") {
        try {
            const Json body = parse_json_body(request);
            const ExportedPayload payload = export_path(body.at("path").get<std::string>());
            log_message(
                LOG_INFO,
                "server",
                "transfer/export path=`" + body.at("path").get<std::string>() +
                    "` source_type=`" + payload.source_type + "`"
            );
            response.status = 200;
            response.headers["Content-Type"] = "application/octet-stream";
            response.headers["x-remote-exec-source-type"] = payload.source_type;
            response.body = payload.bytes;
        } catch (const std::exception& ex) {
            write_rpc_error(response, 400, "transfer_failed", ex.what());
        }
        return response;
    }

    if (request.path == "/v1/transfer/import") {
        try {
            const std::string source_type = request.header("x-remote-exec-source-type");
            const ImportSummary summary = import_path(
                request.body,
                source_type,
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
            write_rpc_error(response, 400, "transfer_failed", ex.what());
        }
        return response;
    }

    write_rpc_error(response, 404, "not_found", "unknown endpoint");
    return response;
}

static SOCKET create_listener(const DaemonConfig& config) {
    char port_buffer[16];
    std::snprintf(port_buffer, sizeof(port_buffer), "%d", config.listen_port);

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;
    hints.ai_flags = AI_PASSIVE;

    addrinfo* result = NULL;
    if (getaddrinfo(config.listen_host.c_str(), port_buffer, &hints, &result) != 0) {
        throw std::runtime_error("getaddrinfo failed");
    }

    SOCKET listener = socket(result->ai_family, result->ai_socktype, result->ai_protocol);
    if (listener == INVALID_SOCKET) {
        freeaddrinfo(result);
        throw std::runtime_error("socket failed");
    }

    const char yes = 1;
    setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &yes, sizeof(yes));

    if (bind(listener, result->ai_addr, static_cast<int>(result->ai_addrlen)) != 0) {
        freeaddrinfo(result);
        closesocket(listener);
        throw std::runtime_error("bind failed");
    }
    freeaddrinfo(result);

    if (listen(listener, SOMAXCONN) != 0) {
        closesocket(listener);
        throw std::runtime_error("listen failed");
    }

    return listener;
}

int run_server(const DaemonConfig& config) {
    WSADATA wsa_data;
    if (WSAStartup(MAKEWORD(2, 2), &wsa_data) != 0) {
        throw std::runtime_error("WSAStartup failed");
    }

    AppState state;
    state.config = config;
    state.daemon_instance_id = daemon_instance_id();
    state.hostname = hostname_string();
    SOCKET listener = INVALID_SOCKET;

    try {
        listener = create_listener(state.config);
        {
            std::ostringstream message;
            message << "listening on " << state.config.listen_host << ':'
                    << state.config.listen_port
                    << " target=`" << state.config.target << "`"
                    << " daemon_instance_id=`" << state.daemon_instance_id << "`";
            log_message(LOG_INFO, "server", message.str());
        }
        for (;;) {
            SOCKET client = accept(listener, NULL, NULL);
            if (client == INVALID_SOCKET) {
                log_message(LOG_WARN, "server", "accept failed");
                continue;
            }

            try {
                const DWORD started_at_ms = GetTickCount();
                const std::string raw_request = read_request(client);
                const HttpRequest request = parse_http_request(raw_request);
                const HttpResponse response = route_request(state, request);
                {
                    std::ostringstream message;
                    message << request.method << ' ' << request.path
                            << " status=" << response.status
                            << " elapsed_ms=" << (GetTickCount() - started_at_ms);
                    log_message(level_for_status(response.status), "server", message.str());
                }
                send_all(client, render_http_response(response));
            } catch (const std::exception& ex) {
                log_message(LOG_ERROR, "server", ex.what());
                HttpResponse response;
                response.status = 500;
                write_rpc_error(response, 500, "internal_error", ex.what());
                send_all(client, render_http_response(response));
            }

            closesocket(client);
        }
    } catch (...) {
        if (listener != INVALID_SOCKET) {
            closesocket(listener);
        }
        WSACleanup();
        throw;
    }

    return 0;
}
