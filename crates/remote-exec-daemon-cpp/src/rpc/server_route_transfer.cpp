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

class HttpBodyTransferStreamReader : public TransferStreamByteReader {
public:
    explicit HttpBodyTransferStreamReader(HttpRequestBodyStream* body) : body_(body) {}

    std::size_t read(char* data, std::size_t size) override {
        try {
            return body_->read(data, size);
        } catch (const BadHttpRequest& ex) {
            throw TransferFailure(TransferRpcCode::TransferFailed, ex.what());
        }
    }

private:
    HttpRequestBodyStream* body_;
};

class HttpChunkedTransferStreamWriter : public TransferStreamChunkWriter {
public:
    explicit HttpChunkedTransferStreamWriter(HttpChunkedResponseWriter* chunks) : chunks_(chunks) {}

    void write_chunk(const char* data, std::size_t size) override { chunks_->write_chunk(data, size); }

    void finish() override { chunks_->finish(); }

private:
    HttpChunkedResponseWriter* chunks_;
};

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
    StringTransferStreamByteReader body_reader(&body);
    TransferStreamArchiveReader archive_reader(&body_reader);
    return run_transfer_import(import_request, archive_reader);
}

void write_transfer_import_success(HttpResponse& response,
                                   const TransferImportRequestSpec& import_request,
                                   const ImportSummary& summary) {
    log_transfer_import_summary(import_request.destination_path, summary);
    write_json(response, transfer_summary_json(summary));
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
        HttpBodyTransferStreamReader body_reader(body);
        TransferStreamArchiveReader archive_reader(&body_reader);
        write_transfer_import_success(route_response,
                                      import_request,
                                      run_transfer_import(import_request, archive_reader));
    });
}

HttpResponse prepare_streaming_transfer_export(const AppState& state,
                                               const HttpRequest& request_head,
                                               HttpRequestBodyStream* body,
                                               StreamingTransferExport* transfer) {
    HttpResponse response;
    response.status = 200;
    if (reject_before_route(state, request_head, &response)) {
        return response;
    }

    return handle_transfer_rpc_route("transfer/export", [&](HttpResponse& route_response) {
        HttpRequest request = request_head;
        require_transfer_stream_version(request);
        request.body = read_request_body_to_string(body);
        const Json body_json = parse_json_body(request);
        transfer->request = prepare_transfer_export_request(state, body_json);
        transfer->response_payload = ExportedPayload{transfer->request.source_type, std::string()};
        route_response.headers["Transfer-Encoding"] = "chunked";
        write_transfer_export_headers(route_response, transfer->response_payload);
    });
}

void run_streaming_transfer_export(const StreamingTransferExport& transfer, HttpChunkedResponseWriter* chunks) {
    HttpChunkedTransferStreamWriter chunk_writer(chunks);
    ChunkedTransferStreamArchiveSink sink(&chunk_writer);
    sink.send_preface();
    try {
        export_path_to_sink_as(sink,
                               transfer.request.path,
                               transfer.request.source_type,
                               transfer.request.symlink_mode,
                               transfer.request.exclude,
                               transfer.request.authorizer);
        log_transfer_export_success(transfer.request, transfer.response_payload);
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
}
