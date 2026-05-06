#include "server_route_common.h"
#include "server_route_exec.h"
#include "server_route_image.h"
#include "server_route_transfer.h"
#include "server_request_utils.h"
#include "server_routes.h"

HttpResponse route_request(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;
    if (reject_before_route(state, request, &response)) {
        return response;
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
