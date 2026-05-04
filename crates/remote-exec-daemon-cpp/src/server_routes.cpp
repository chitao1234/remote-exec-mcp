#include "server_route_common.h"
#include "server_route_exec.h"
#include "server_route_image.h"
#include "server_route_port_forward.h"
#include "server_route_transfer.h"
#include "server_routes.h"

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
