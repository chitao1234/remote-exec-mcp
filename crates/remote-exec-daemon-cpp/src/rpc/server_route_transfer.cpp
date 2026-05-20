#include <sstream>
#include <string>

#include "core/logging.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_transfer.h"
#include "rpc/transfer_http_codec.h"
#include "rpc/transfer_request_utils.h"
#include "rpc/transfer_stream_codec.h"
#include "rpc/transfer_stream_io.h"

namespace {

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

void log_transfer_export_success(const TransferExportRequestSpec& export_request, const ExportedPayload& payload) {
    log_message(LOG_INFO,
                "server",
                "transfer/export path=`" + export_request.path + "` source_type=`" +
                    transfer_source_type_wire_value(payload.source_type) + "`");
}

void write_transfer_export_success(HttpResponse& response,
                                   const TransferExportRequestSpec& export_request,
                                   const ExportedPayload& payload) {
    log_transfer_export_success(export_request, payload);
    write_transfer_export_headers(response, payload);
    response.body = framed_transfer_body(payload.bytes);
}

ImportSummary run_transfer_import(const TransferImportRequestSpec& import_request,
                                  TransferArchiveReader& archive_reader) {
    return import_path_from_reader(archive_reader,
                                   import_request.metadata.source_type,
                                   import_request.destination_path,
                                   import_request.metadata.overwrite,
                                   import_request.metadata.create_parent,
                                   import_request.metadata.symlink_mode,
                                   import_request.limits,
                                   import_request.authorizer);
}

ImportSummary run_transfer_import(const TransferImportRequestSpec& import_request, const std::string& body) {
    StringTransferStreamArchiveReader archive_reader(&body);
    return run_transfer_import(import_request, archive_reader);
}

void write_transfer_import_success(HttpResponse& response,
                                   const TransferImportRequestSpec& import_request,
                                   const ImportSummary& summary) {
    log_transfer_import_summary(import_request.destination_path, summary);
    write_json(response, transfer_summary_json(summary));
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

bool try_send_streaming_transfer_response(SOCKET client, const HttpResponse& response) {
    try {
        send_all(client, render_http_response(response));
        return true;
    } catch (const SocketSendError& ex) {
        if (ex.peer_disconnected()) {
            log_message(LOG_WARN, "server", std::string("client disconnected during send: ") + ex.what());
            return false;
        }
        log_message(LOG_ERROR, "server", std::string("send failed: ") + ex.what());
        return false;
    }
}

} // namespace

HttpResponse handle_transfer_export(AppState& state, const HttpRequest& request) {
    return handle_transfer_rpc_route("transfer/export", [&](HttpResponse& response) {
        require_transfer_stream_version(request);
        const Json body = parse_json_body(request);
        const TransferExportRequestSpec export_request = prepare_transfer_export_request(state, body);
        const ExportedPayload payload = export_path(
            export_request.path, export_request.symlink_mode, export_request.exclude, export_request.authorizer);
        write_transfer_export_success(response, export_request, payload);
    });
}

HttpResponse handle_transfer_path_info(AppState& state, const HttpRequest& request) {
    return handle_transfer_rpc_route("transfer/path-info", [&](HttpResponse& response) {
        const Json body = parse_json_body(request);
        const std::string path =
            resolve_authorized_transfer_path(state, body.at("path").get<std::string>(), SANDBOX_WRITE);
        const PathInfo info = path_info(path);
        write_json(response,
                   Json{
                       {"exists", info.exists},
                       {"is_directory", info.is_directory},
                   });
    });
}

HttpResponse handle_transfer_import(AppState& state, const HttpRequest& request) {
    return handle_transfer_rpc_route("transfer/import", [&](HttpResponse& response) {
        require_transfer_stream_version(request);
        require_transfer_stream_content_type(request);
        const TransferImportRequestSpec import_request = prepare_transfer_import_request(state, request);
        write_transfer_import_success(response, import_request, run_transfer_import(import_request, request.body));
    });
}

HttpResponse
handle_streaming_transfer_import(const AppState& state, const HttpRequest& request, HttpRequestBodyStream* body) {
    HttpResponse response;
    response.status = 200;

    if (reject_before_route(state, request, &response)) {
        return response;
    }

    return handle_transfer_rpc_route("transfer/import", [&](HttpResponse& route_response) {
        require_transfer_stream_version(request);
        require_transfer_stream_content_type(request);
        const TransferImportRequestSpec import_request = prepare_transfer_import_request(state, request);
        HttpBodyTransferArchiveReader archive_reader(body);
        write_transfer_import_success(route_response,
                                      import_request,
                                      run_transfer_import(import_request, archive_reader));
    });
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

    try {
        HttpRequest request = request_head;
        require_transfer_stream_version(request);
        request.body = read_request_body_to_string(body);
        const Json body_json = parse_json_body(request);
        const TransferExportRequestSpec export_request = prepare_transfer_export_request(state, body_json);
        const ExportedPayload payload = ExportedPayload{export_request.source_type, std::string()};
        send_transfer_export_headers(client, payload, request);

        ChunkedTransferStreamArchiveSink sink(client);
        sink.send_preface();
        try {
            export_path_to_sink_as(sink,
                                   export_request.path,
                                   export_request.source_type,
                                   export_request.symlink_mode,
                                   export_request.exclude,
                                   export_request.authorizer);
            log_transfer_export_success(export_request, payload);
            sink.send_complete();
        } catch (const SandboxError& ex) {
            const std::string message = ex.what();
            log_message(LOG_WARN, "server", "transfer/export failed after stream start: " + message);
            sink.send_error_payload(transfer_stream::error_payload(TransferRpcCode::SandboxDenied, ex.what()));
        } catch (const TransferFailure& failure) {
            log_message(LOG_WARN, "server", "transfer/export failed after stream start: " + failure.message);
            sink.send_error_payload(transfer_stream::error_payload(failure));
        } catch (const SocketSendError&) {
            throw;
        } catch (const std::exception& ex) {
            const std::string message = ex.what();
            log_message(LOG_WARN, "server", "transfer/export failed after stream start: " + message);
            sink.send_error_payload(transfer_stream::error_payload(ex));
        }
        return 200;
    } catch (const SandboxError& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        HttpResponse response;
        response.status = 400;
        write_transfer_error_response(response, ex);
        write_request_id_header(response, request_head);
        try_send_streaming_transfer_response(client, response);
        return response.status;
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
        HttpResponse response;
        response.status = transfer_error_status(failure.code);
        write_transfer_error_response(response, failure);
        write_request_id_header(response, request_head);
        try_send_streaming_transfer_response(client, response);
        return response.status;
    } catch (const SocketSendError&) {
        throw;
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        HttpResponse response;
        response.status = transfer_error_status(TransferRpcCode::Internal);
        write_transfer_internal_error_response(response, message);
        write_request_id_header(response, request_head);
        try_send_streaming_transfer_response(client, response);
        return response.status;
    }
}
