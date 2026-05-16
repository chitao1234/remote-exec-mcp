#include <cstddef>

#include "server_contract.h"
#include "server_routes.h"
#include "server_request_utils.h"
#include "server_route_common.h"
#include "server_route_exec.h"
#include "server_route_image.h"
#include "server_route_transfer.h"

namespace {

typedef HttpResponse (*RouteHandler)(AppState& state, const HttpRequest& request);

HttpResponse route_health(AppState& state, const HttpRequest&) {
    return handle_health(state);
}

HttpResponse route_target_info(AppState& state, const HttpRequest&) {
    return handle_target_info(state);
}

struct RouteDispatchEntry {
    server_contract::RouteId id;
    RouteHandler handler;
};

const RouteDispatchEntry ROUTE_DISPATCH[] = {
    {server_contract::ROUTE_HEALTH, &route_health},
    {server_contract::ROUTE_TARGET_INFO, &route_target_info},
    {server_contract::ROUTE_IMAGE_READ, &handle_image_read},
    {server_contract::ROUTE_EXEC_START, &handle_exec_start},
    {server_contract::ROUTE_EXEC_WRITE, &handle_exec_write},
    {server_contract::ROUTE_PATCH_APPLY, &handle_patch_apply},
    {server_contract::ROUTE_TRANSFER_EXPORT, &handle_transfer_export},
    {server_contract::ROUTE_TRANSFER_PATH_INFO, &handle_transfer_path_info},
    {server_contract::ROUTE_TRANSFER_IMPORT, &handle_transfer_import},
};

const std::size_t ROUTE_DISPATCH_COUNT = sizeof(ROUTE_DISPATCH) / sizeof(ROUTE_DISPATCH[0]);

RouteHandler find_route_handler(server_contract::RouteId id) {
    for (std::size_t i = 0; i < ROUTE_DISPATCH_COUNT; ++i) {
        if (ROUTE_DISPATCH[i].id == id) {
            return ROUTE_DISPATCH[i].handler;
        }
    }
    return nullptr;
}

} // namespace

HttpResponse route_request(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;
    if (reject_before_route(state, request, &response)) {
        write_request_id_header(response, request);
        return response;
    }

    const RouteHandler handler = find_route_handler(server_contract::route_id_for_path(request.path));
    if (handler != nullptr) {
        response = handler(state, request);
    } else {
        response = make_rpc_error_response(404, "not_found", "unknown endpoint");
    }

    write_request_id_header(response, request);
    return response;
}
