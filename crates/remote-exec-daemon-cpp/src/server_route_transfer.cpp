#include <string>

#include "logging.h"
#include "server_request_utils.h"
#include "server_route_transfer.h"
#include "transfer_http_codec.h"

HttpResponse handle_transfer_export(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const TransferExportRequestSpec export_request = prepare_transfer_export_request(state, body);
        const ExportedPayload payload =
            export_path(export_request.path, export_request.symlink_mode, export_request.exclude);
        log_message(LOG_INFO,
                    "server",
                    "transfer/export path=`" + export_request.path + "` source_type=`" +
                        transfer_source_type_wire_value(export_request.source_type) + "`");
        write_transfer_export_headers(response, ExportedPayload{export_request.source_type, payload.bytes});
        response.body = payload.bytes;
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("transfer/export failed: ") + ex.what());
        write_transfer_error_response(response, ex);
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
        write_transfer_error_response(response, failure);
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/export failed: " + message);
        write_transfer_internal_error_response(response, message);
    }

    return response;
}

HttpResponse handle_transfer_path_info(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const Json body = parse_json_body(request);
        const std::string path =
            resolve_authorized_transfer_path(state, body.at("path").get<std::string>(), SANDBOX_WRITE);
        const PathInfo info = path_info(path);
        write_json(response,
                   Json{
                       {"exists", info.exists},
                       {"is_directory", info.is_directory},
                   });
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("transfer/path-info failed: ") + ex.what());
        write_transfer_error_response(response, ex);
    } catch (const TransferFailure& failure) {
        log_message(LOG_WARN, "server", "transfer/path-info failed: " + failure.message);
        write_transfer_error_response(response, failure);
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_WARN, "server", "transfer/path-info failed: " + message);
        write_transfer_internal_error_response(response, message);
    }

    return response;
}

HttpResponse handle_transfer_import(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const TransferImportRequestSpec import_request = prepare_transfer_import_request(state, request);
        const ImportSummary summary = import_path(request.body,
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
