#include <cstdint>
#include <sstream>
#include <string>
#include <vector>

#include "http_helpers.h"
#include "http_request.h"
#include "logging.h"
#include "platform.h"
#include "port_tunnel.h"
#include "server.h"
#include "server_request_utils.h"
#include "server_route_common.h"
#include "server_routes.h"
#include "server_transport.h"
#include "text_utils.h"
#include "transfer_http_codec.h"
#include "transfer_ops.h"

namespace {

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

bool request_connection_close_requested(const HttpRequest& request) {
    const std::string value = lowercase_ascii(request.header("connection"));
    std::size_t offset = 0;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token =
            trim_ascii(comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset));
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

HttpRequestBodyFraming parse_request_body_framing_or_throw_bad_request(const HttpRequest& request) {
    try {
        return request_body_framing_from_headers(request.headers);
    } catch (const HttpProtocolError& ex) {
        throw BadHttpRequest(ex.what());
    }
}

HttpResponse
handle_streaming_transfer_import(const AppState& state, const HttpRequest& request, HttpRequestBodyStream* body) {
    HttpResponse response;
    response.status = 200;

    if (reject_before_route(state, request, &response)) {
        return response;
    }

    try {
        const TransferImportRequestSpec import_request = prepare_transfer_import_request(state, request);
        HttpBodyTransferArchiveReader archive_reader(body);
        const ImportSummary summary = import_path_from_reader(archive_reader,
                                                              import_request.metadata.source_type,
                                                              import_request.destination_path,
                                                              import_request.metadata.overwrite,
                                                              import_request.metadata.create_parent,
                                                              import_request.metadata.symlink_mode,
                                                              import_request.limits,
                                                              import_request.authorizer);
        log_transfer_import_summary(import_request.destination_path, summary);
        write_json(response, transfer_summary_json(summary));
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("transfer/import failed: ") + ex.what());
        write_transfer_error_response(response, ex);
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/import failed: " + failure.message);
        write_transfer_error_response(response, failure);
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/import failed: " + message);
        write_transfer_internal_error_response(response, message);
    }

    return response;
}

void send_transfer_export_headers(SOCKET client, const ExportedPayload& payload, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;
    response.headers["Transfer-Encoding"] = "chunked";
    write_transfer_export_headers(response, payload);
    write_request_id_header(response, request);

    std::ostringstream out;
    out << "HTTP/1.1 200 OK\r\n";
    for (std::map<std::string, std::string>::const_iterator it = response.headers.begin(); it != response.headers.end();
         ++it) {
        out << it->first << ": " << it->second << "\r\n";
    }
    out << "\r\n";
    send_all(client, out.str());
}

int handle_streaming_transfer_export(const AppState& state,
                                     const HttpRequest& request_head,
                                     HttpRequestBodyStream* body,
                                     SOCKET client) {
    HttpResponse rejection;
    rejection.status = 200;
    if (reject_before_route(state, request_head, &rejection)) {
        write_request_id_header(rejection, request_head);
        send_all(client, render_http_response(rejection));
        return rejection.status;
    }

    bool headers_sent = false;
    try {
        HttpRequest request = request_head;
        request.body = read_request_body_to_string(body);
        const Json body_json = parse_json_body(request);
        const TransferExportRequestSpec export_request = prepare_transfer_export_request(state, body_json);
        log_message(LOG_INFO,
                    "server",
                    "transfer/export path=`" + export_request.path + "` source_type=`" +
                        transfer_source_type_wire_value(export_request.source_type) + "`");

        send_transfer_export_headers(client, ExportedPayload{export_request.source_type, std::string()}, request);
        headers_sent = true;
        ChunkedTransferArchiveSink sink(client);
        export_path_to_sink_as(
            sink, export_request.path, export_request.source_type, export_request.symlink_mode, export_request.exclude);
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
        write_transfer_error_response(response, ex);
        write_request_id_header(response, request_head);
        try_send_response(client, response);
        return response.status;
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
        if (headers_sent) {
            return 200;
        }

        HttpResponse response;
        response.status = transfer_error_status(failure.code);
        write_transfer_error_response(response, failure);
        write_request_id_header(response, request_head);
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
        write_transfer_internal_error_response(response, message);
        write_request_id_header(response, request_head);
        try_send_response(client, response);
        return response.status;
    }
}

void log_request_result(const HttpRequest& request, int status, std::uint64_t started_at_ms) {
    LogMessageBuilder message(request.method + " " + request.path);
    message.field("request_id", request_id_for_request(request))
        .field("status", status)
        .field("elapsed_ms", (platform::monotonic_ms() - started_at_ms));
    log_message(level_for_status(status), "server", message.str());
}

int handle_client_request(AppState& state,
                          SOCKET client,
                          const HttpRequestHead& request_head,
                          bool* close_after_response) {
    const std::uint64_t started_at_ms = platform::monotonic_ms();
    HttpRequest request = parse_http_request_head(request_head.raw_headers);
    assign_request_id(request);
    *close_after_response = request_connection_close_requested(request);
    const HttpRequestBodyFraming framing = parse_request_body_framing_or_throw_bad_request(request);
    HttpRequestBodyStream body(client, request_head.initial_body, framing, state.config.max_request_body_bytes);

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
    write_request_id_header(response, request);
    log_request_result(request, response.status, started_at_ms);
    if (!try_send_response(client, response)) {
        *close_after_response = true;
    }
    return response.status;
}

} // namespace

void handle_client(AppState& state, UniqueSocket client) {
    for (;;) {
        try {
            set_socket_timeout_ms(client.get(), state.config.http_connection_idle_timeout_ms);
            HttpRequestHead request_head;
            if (!try_read_http_request_head(client.get(), state.config.max_request_header_bytes, &request_head)) {
                return;
            }
            set_socket_timeout_ms(client.get(), 0UL);

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
