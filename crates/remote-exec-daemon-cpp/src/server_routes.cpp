#include "server_routes.h"
#include "server_request_utils.h"
#include "server_route_common.h"
#include "server_route_exec.h"
#include "server_route_image.h"
#include "server_route_transfer.h"

HttpResponse route_request(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;
    if (reject_before_route(state, request, &response)) {
        write_request_id_header(response, request);
        return response;
    }

    if (request.path == "/v1/health") {
        response = handle_health(state);
    } else if (request.path == "/v1/target-info") {
        response = handle_target_info(state);
    } else if (request.path == "/v1/image/read") {
        response = handle_image_read(state, request);
    } else if (request.path == "/v1/exec/start") {
        response = handle_exec_start(state, request);
    } else if (request.path == "/v1/exec/write") {
        response = handle_exec_write(state, request);
    } else if (request.path == "/v1/patch/apply") {
        response = handle_patch_apply(state, request);
    } else if (request.path == "/v1/transfer/export") {
        response = handle_transfer_export(state, request);
    } else if (request.path == "/v1/transfer/path-info") {
        response = handle_transfer_path_info(state, request);
    } else if (request.path == "/v1/transfer/import") {
        response = handle_transfer_import(state, request);
    } else {
        response = make_rpc_error_response(404, "not_found", "unknown endpoint");
    }

    write_request_id_header(response, request);
    return response;
}
