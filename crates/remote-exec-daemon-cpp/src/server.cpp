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
#include "port_tunnel.h"
#include "rpc_failures.h"
#include "server_route_common.h"
#include "server.h"
#include "server_routes.h"
#include "server_transport.h"
#include "text_utils.h"
#include "transfer_http_codec.h"
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

std::vector<std::string> transfer_exclude_or_empty(const Json& body) {
    const Json::const_iterator it = body.find("exclude");
    if (it == body.end() || it->is_null()) {
        return std::vector<std::string>();
    }
    return it->get<std::vector<std::string> >();
}

bool request_connection_close_requested(const HttpRequest& request) {
    const std::string value = lowercase_ascii(request.header("connection"));
    std::size_t offset = 0;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos
                ? value.substr(offset)
                : value.substr(offset, comma - offset)
        );
        if (token == "close") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }

    return false;
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

bool log_send_failure(const SocketSendError& ex);
bool try_send_response(SOCKET client, const HttpResponse& response);

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
        const TransferImportMetadata metadata = parse_transfer_import_metadata(request);
        require_uncompressed_transfer(metadata.compression);
        const std::string destination_path =
            resolve_absolute_transfer_path(metadata.destination_path);
        authorize_sandbox_path(state, SANDBOX_WRITE, destination_path);
        HttpBodyTransferArchiveReader archive_reader(body);
        const ImportSummary summary = import_path_from_reader(
            archive_reader,
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

void send_transfer_export_headers(SOCKET client, const ExportedPayload& payload) {
    HttpResponse response;
    response.status = 200;
    response.headers["Transfer-Encoding"] = "chunked";
    write_transfer_export_headers(response, payload);

    std::ostringstream out;
    out << "HTTP/1.1 200 OK\r\n";
    for (std::map<std::string, std::string>::const_iterator it = response.headers.begin();
         it != response.headers.end();
         ++it) {
        out << it->first << ": " << it->second << "\r\n";
    }
    out << "\r\n";
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
        const std::vector<std::string> exclude = transfer_exclude_or_empty(body_json);
        const std::string source_type = export_path_source_type(path, symlink_mode);
        log_message(
            LOG_INFO,
            "server",
            "transfer/export path=`" + path + "` source_type=`" + source_type + "`"
        );

        send_transfer_export_headers(client, ExportedPayload{source_type, std::string()});
        headers_sent = true;
        ChunkedTransferArchiveSink sink(client);
        export_path_to_sink_as(sink, path, source_type, symlink_mode, exclude);
        sink.finish();
        return 200;
    } catch (const SandboxError& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        if (headers_sent) {
            return 200;
        }

        HttpResponse response;
        response.status = 400;
        write_rpc_error(
            response,
            400,
            transfer_error_code_name(TransferRpcCode::SandboxDenied),
            message
        );
        try_send_response(client, response);
        return response.status;
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
        if (headers_sent) {
            return 200;
        }

        HttpResponse response;
        response.status = transfer_error_status(failure.code);
        write_rpc_error(
            response,
            transfer_error_status(failure.code),
            transfer_error_code_name(failure.code),
            failure.message
        );
        try_send_response(client, response);
        return response.status;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        if (headers_sent) {
            return 200;
        }

        HttpResponse response;
        response.status = transfer_error_status(TransferRpcCode::Internal);
        write_rpc_error(
            response,
            transfer_error_status(TransferRpcCode::Internal),
            transfer_error_code_name(TransferRpcCode::Internal),
            message
        );
        try_send_response(client, response);
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

bool log_send_failure(const SocketSendError& ex) {
    if (ex.peer_disconnected()) {
        log_message(LOG_WARN, "server", std::string("client disconnected during send: ") + ex.what());
        return true;
    }

    log_message(LOG_ERROR, "server", std::string("send failed: ") + ex.what());
    return false;
}

bool try_send_response(SOCKET client, const HttpResponse& response) {
    try {
        send_all(client, render_http_response(response));
        return true;
    } catch (const SocketSendError& ex) {
        log_send_failure(ex);
        return false;
    }
}

}  // namespace

int handle_client_request(
    AppState& state,
    SOCKET client,
    const HttpRequestHead& request_head,
    bool* close_after_response
) {
    const std::uint64_t started_at_ms = platform::monotonic_ms();
    HttpRequest request = parse_http_request_head(request_head.raw_headers);
    *close_after_response = request_connection_close_requested(request);
    const HttpRequestBodyFraming framing =
        request_body_framing_from_headers(request_head.raw_headers);
    HttpRequestBodyStream body(
        client,
        request_head.initial_body,
        framing,
        state.config.max_request_body_bytes
    );

    if (request.path == "/v1/transfer/export") {
        const int status = handle_streaming_transfer_export(state, request, &body, client);
        log_request_result(request, status, started_at_ms);
        return status;
    }
    if (is_port_tunnel_upgrade_request(request)) {
        const int status = handle_port_tunnel_upgrade(state, client, request);
        log_request_result(request, status, started_at_ms);
        *close_after_response = true;
        return status;
    }

    HttpResponse response;
    if (request.path == "/v1/transfer/import") {
        response = handle_streaming_transfer_import(state, request, &body);
    } else {
        request.body = read_request_body_to_string(&body);
        response = route_request(state, request);
    }
    log_request_result(request, response.status, started_at_ms);
    if (!try_send_response(client, response)) {
        *close_after_response = true;
    }
    return response.status;
}

void handle_client(AppState& state, UniqueSocket client) {
    for (;;) {
        try {
            HttpRequestHead request_head;
            if (!try_read_http_request_head(
                    client.get(),
                    state.config.max_request_header_bytes,
                    &request_head
                )) {
                return;
            }

            bool close_after_response = false;
            handle_client_request(state, client.get(), request_head, &close_after_response);
            if (close_after_response) {
                return;
            }
        } catch (const BadHttpRequest& ex) {
            log_message(LOG_WARN, "server", ex.what());
            HttpResponse response;
            response.status = 400;
            write_rpc_error(response, 400, "bad_request", ex.what());
            try_send_response(client.get(), response);
            return;
        } catch (const HttpParseError& ex) {
            log_message(LOG_WARN, "server", ex.what());
            HttpResponse response;
            response.status = 400;
            write_rpc_error(response, 400, "bad_request", ex.what());
            try_send_response(client.get(), response);
            return;
        } catch (const SocketSendError& ex) {
            log_send_failure(ex);
            return;
        } catch (const std::exception& ex) {
            log_message(LOG_ERROR, "server", ex.what());
            HttpResponse response;
            response.status = 500;
            write_rpc_error(response, 500, "internal_error", ex.what());
            try_send_response(client.get(), response);
            return;
        }
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
    handle_client(*context->state, std::move(client));
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
            handle_client(state, std::move(thread_client));
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
