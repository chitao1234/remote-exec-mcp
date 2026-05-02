#include <cstdint>
#include <memory>
#include <sstream>
#include <string>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <thread>
#endif

#include "common.h"
#include "filesystem_sandbox.h"
#include "http_request.h"
#include "http_helpers.h"
#include "logging.h"
#include "path_policy.h"
#include "platform.h"
#include "server.h"
#include "server_routes.h"
#include "server_transport.h"
#include "transfer_ops.h"

namespace {

std::string daemon_instance_id() {
    std::ostringstream out;
    out << platform::monotonic_ms();
    return out.str();
}

class HttpBodyTransferArchiveReader : public TransferArchiveReader {
public:
    explicit HttpBodyTransferArchiveReader(HttpRequestBodyStream* body) : body_(body) {}

    bool read_exact_or_eof(char* data, std::size_t size) {
        std::size_t offset = 0;
        while (offset < size) {
            const std::size_t received = body_->read(data + offset, size - offset);
            if (received == 0U) {
                if (offset == 0U) {
                    return false;
                }
                throw std::runtime_error("truncated transfer body");
            }
            offset += received;
        }
        return true;
    }

private:
    HttpRequestBodyStream* body_;
};

class ChunkedTransferArchiveSink : public TransferArchiveSink {
public:
    explicit ChunkedTransferArchiveSink(SOCKET client) : client_(client), finished_(false) {}

    void write(const char* data, std::size_t size) {
        if (size == 0U) {
            return;
        }
        std::ostringstream header;
        header << std::hex << size << "\r\n";
        send_all(client_, header.str());
        send_all_bytes(client_, data, size);
        send_all(client_, "\r\n");
    }

    void finish() {
        if (finished_) {
            return;
        }
        send_all(client_, "0\r\n\r\n");
        finished_ = true;
    }

private:
    SOCKET client_;
    bool finished_;
};

std::string read_request_body_to_string(HttpRequestBodyStream* body) {
    std::string output;
    char buffer[8192];
    for (;;) {
        const std::size_t received = body->read(buffer, sizeof(buffer));
        if (received == 0U) {
            return output;
        }
        output.append(buffer, received);
    }
}

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
        throw std::runtime_error("transfer path is not absolute");
    }
    return normalize_for_system(policy, path);
}

HttpResponse handle_streaming_transfer_import(
    const AppState& state,
    const HttpRequest& request,
    HttpRequestBodyStream* body
) {
    HttpResponse response;
    response.status = 200;

    if (reject_before_route(state, request, &response)) {
        return response;
    }

    try {
        require_uncompressed_transfer(request.header("x-remote-exec-compression"));
        const std::string destination_path =
            resolve_absolute_transfer_path(request.header("x-remote-exec-destination-path"));
        authorize_sandbox_path(state, SANDBOX_WRITE, destination_path);
        HttpBodyTransferArchiveReader archive_reader(body);
        const ImportSummary summary = import_path_from_reader(
            archive_reader,
            request.header("x-remote-exec-source-type"),
            destination_path,
            request.header("x-remote-exec-overwrite"),
            request.header("x-remote-exec-create-parent") == "true",
            transfer_symlink_mode_or_default(request.header("x-remote-exec-symlink-mode"))
        );
        log_transfer_import_summary(destination_path, summary);
        write_json(response, transfer_summary_json(summary));
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/import failed: " + message);
        write_rpc_error(response, 400, transfer_error_code(message), message);
    }

    return response;
}

void send_transfer_export_headers(SOCKET client, const std::string& source_type) {
    std::ostringstream out;
    out << "HTTP/1.1 200 OK\r\n"
        << "Connection: close\r\n"
        << "Content-Type: application/octet-stream\r\n"
        << "Transfer-Encoding: chunked\r\n"
        << "x-remote-exec-compression: none\r\n"
        << "x-remote-exec-source-type: " << source_type << "\r\n"
        << "\r\n";
    send_all(client, out.str());
}

int handle_streaming_transfer_export(
    const AppState& state,
    const HttpRequest& request_head,
    HttpRequestBodyStream* body,
    SOCKET client
) {
    HttpResponse rejection;
    rejection.status = 200;
    if (reject_before_route(state, request_head, &rejection)) {
        send_all(client, render_http_response(rejection));
        return rejection.status;
    }

    bool headers_sent = false;
    try {
        HttpRequest request = request_head;
        request.body = read_request_body_to_string(body);
        const Json body_json = parse_json_body(request);
        require_uncompressed_transfer(body_json.value("compression", std::string("none")));

        const std::string path =
            resolve_absolute_transfer_path(body_json.at("path").get<std::string>());
        authorize_sandbox_path(state, SANDBOX_READ, path);
        const std::string symlink_mode = body_json.value("symlink_mode", std::string("preserve"));
        const std::string source_type = export_path_source_type(path, symlink_mode);
        log_message(
            LOG_INFO,
            "server",
            "transfer/export path=`" + path + "` source_type=`" + source_type + "`"
        );

        send_transfer_export_headers(client, source_type);
        headers_sent = true;
        ChunkedTransferArchiveSink sink(client);
        export_path_to_sink_as(sink, path, source_type, symlink_mode);
        sink.finish();
        return 200;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        if (headers_sent) {
            return 200;
        }

        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, transfer_error_code(message), message);
        send_all(client, render_http_response(response));
        return response.status;
    }
}

void log_request_result(
    const HttpRequest& request,
    int status,
    std::uint64_t started_at_ms
) {
    std::ostringstream message;
    message << request.method << ' ' << request.path
            << " status=" << status
            << " elapsed_ms=" << (platform::monotonic_ms() - started_at_ms);
    log_message(level_for_status(status), "server", message.str());
}

}  // namespace

void handle_client_once(AppState& state, UniqueSocket client) {
    try {
        const std::uint64_t started_at_ms = platform::monotonic_ms();
        const HttpRequestHead request_head = read_http_request_head(
            client.get(),
            state.config.max_request_header_bytes
        );
        HttpRequest request = parse_http_request_head(request_head.raw_headers);
        const HttpRequestBodyFraming framing =
            request_body_framing_from_headers(request_head.raw_headers);
        HttpRequestBodyStream body(
            client.get(),
            request_head.initial_body,
            framing,
            state.config.max_request_body_bytes
        );

        if (request.path == "/v1/transfer/export") {
            const int status = handle_streaming_transfer_export(state, request, &body, client.get());
            log_request_result(request, status, started_at_ms);
            return;
        }

        HttpResponse response;
        if (request.path == "/v1/transfer/import") {
            response = handle_streaming_transfer_import(state, request, &body);
        } else {
            request.body = read_request_body_to_string(&body);
            response = route_request(state, request);
        }
        log_request_result(request, response.status, started_at_ms);
        send_all(client.get(), render_http_response(response));
    } catch (const BadHttpRequest& ex) {
        log_message(LOG_WARN, "server", ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "bad_request", ex.what());
        send_all(client.get(), render_http_response(response));
    } catch (const HttpParseError& ex) {
        log_message(LOG_WARN, "server", ex.what());
        HttpResponse response;
        response.status = 400;
        write_rpc_error(response, 400, "bad_request", ex.what());
        send_all(client.get(), render_http_response(response));
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", ex.what());
        HttpResponse response;
        response.status = 500;
        write_rpc_error(response, 500, "internal_error", ex.what());
        send_all(client.get(), render_http_response(response));
    }
}

namespace {

#ifdef _WIN32
struct ClientThreadContext {
    AppState* state;
    SOCKET socket;
};

DWORD WINAPI client_thread_entry(LPVOID raw_context) {
    std::unique_ptr<ClientThreadContext> context(
        static_cast<ClientThreadContext*>(raw_context)
    );
    UniqueSocket client(context->socket);
    handle_client_once(*context->state, std::move(client));
    return 0;
}
#endif

void spawn_client_thread(AppState& state, UniqueSocket client) {
#ifdef _WIN32
    std::unique_ptr<ClientThreadContext> context(new ClientThreadContext());
    context->state = &state;
    context->socket = client.release();
    HANDLE handle = CreateThread(NULL, 0, client_thread_entry, context.get(), 0, NULL);
    if (handle == NULL) {
        UniqueSocket cleanup(context->socket);
        log_message(LOG_ERROR, "server", "CreateThread failed");
        return;
    }
    context.release();
    CloseHandle(handle);
#else
    std::thread(
        [&state](SOCKET socket) {
            UniqueSocket thread_client(socket);
            handle_client_once(state, std::move(thread_client));
        },
        client.release()
    )
        .detach();
#endif
}

}  // namespace

int run_server(const DaemonConfig& config) {
    NetworkSession network;

    AppState state;
    state.config = config;
    state.daemon_instance_id = daemon_instance_id();
    state.hostname = platform::hostname();
    state.default_shell = platform::resolve_default_shell(config.default_shell);
    state.sandbox_enabled = config.sandbox_configured;
    if (state.sandbox_enabled) {
        state.sandbox = compile_filesystem_sandbox(host_path_policy(), config.sandbox);
    }
    UniqueSocket listener(create_listener(state.config));

    {
        std::ostringstream message;
        message << "listening on " << state.config.listen_host << ':'
                << state.config.listen_port
                << " target=`" << state.config.target << "`"
                << " http_auth_enabled=`"
                << (!state.config.http_auth_bearer_token.empty() ? "true" : "false") << "`"
                << " platform=`" << platform::platform_name() << "`"
                << " arch=`" << platform::arch_name() << "`"
                << " default_shell=`" << state.default_shell << "`"
                << " daemon_instance_id=`" << state.daemon_instance_id << "`";
        log_message(LOG_INFO, "server", message.str());
    }

    for (;;) {
        UniqueSocket client(accept_client(listener.get()));
        if (!client.valid()) {
            log_message(LOG_WARN, "server", "accept failed");
            continue;
        }

        spawn_client_thread(state, std::move(client));
    }

    return 0;
}
