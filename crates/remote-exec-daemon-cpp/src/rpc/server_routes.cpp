#include <cstddef>

#include "rpc/server_contract.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_exec.h"
#include "rpc/server_route_image.h"
#include "rpc/server_route_transfer.h"
#include "rpc/server_routes.h"

namespace {

typedef HttpResponse (*RouteHandler)(const ServerRouteContext& context, const HttpRequest& request);

HttpResponse route_health(const ServerRouteContext& context, const HttpRequest&) {
    return handle_health(context.health);
}

HttpResponse route_target_info(const ServerRouteContext& context, const HttpRequest&) {
    return handle_target_info(context.target_info);
}

HttpResponse route_image_read(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_image_read(context.image, request);
}

HttpResponse route_exec_start(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_exec_start(context.exec, request);
}

HttpResponse route_exec_write(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_exec_write(context.exec, request);
}

HttpResponse route_patch_apply(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_patch_apply(context.patch, request);
}

HttpResponse route_transfer_export(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_transfer_export(context.transfer, request);
}

HttpResponse route_transfer_path_info(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_transfer_path_info(context.transfer, request);
}

HttpResponse route_transfer_import(const ServerRouteContext& context, const HttpRequest& request) {
    return handle_transfer_import(context.transfer, request);
}

struct RouteDispatchEntry {
    server_contract::RouteId id;
    RouteHandler handler;
    RouteExecutionMode mode;
};

const RouteDispatchEntry ROUTE_DISPATCH[] = {
    {server_contract::ROUTE_HEALTH, &route_health, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_TARGET_INFO, &route_target_info, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_IMAGE_READ, &route_image_read, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_EXEC_START, &route_exec_start, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_EXEC_WRITE, &route_exec_write, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_PATCH_APPLY, &route_patch_apply, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_TRANSFER_EXPORT, &route_transfer_export, ROUTE_EXECUTION_STREAMING_EXPORT},
    {server_contract::ROUTE_TRANSFER_PATH_INFO, &route_transfer_path_info, ROUTE_EXECUTION_BUFFERED},
    {server_contract::ROUTE_TRANSFER_IMPORT, &route_transfer_import, ROUTE_EXECUTION_STREAMING_IMPORT},
    {server_contract::ROUTE_PORT_TUNNEL, nullptr, ROUTE_EXECUTION_UPGRADE},
};

const std::size_t ROUTE_DISPATCH_COUNT = sizeof(ROUTE_DISPATCH) / sizeof(ROUTE_DISPATCH[0]);

const RouteDispatchEntry* find_route_entry(server_contract::RouteId id) {
    for (std::size_t i = 0; i < ROUTE_DISPATCH_COUNT; ++i) {
        if (ROUTE_DISPATCH[i].id == id) {
            return &ROUTE_DISPATCH[i];
        }
    }
    return nullptr;
}

} // namespace

RouteExecutionMode route_execution_mode(const HttpRequest& request) {
    const RouteDispatchEntry* entry = find_route_entry(server_contract::route_id_for_path(request.path));
    return entry == nullptr ? ROUTE_EXECUTION_BUFFERED : entry->mode;
}

HttpResponse route_request(const ServerRouteContext& context, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;
    if (reject_before_route(context.gate, request, &response)) {
        write_request_id_header(response, request);
        return response;
    }

    const RouteDispatchEntry* entry = find_route_entry(server_contract::route_id_for_path(request.path));
    if (entry != nullptr && entry->handler != nullptr) {
        response = entry->handler(context, request);
    } else {
        response = make_rpc_error_response(404, "not_found", "unknown endpoint");
    }

    write_request_id_header(response, request);
    return response;
}
