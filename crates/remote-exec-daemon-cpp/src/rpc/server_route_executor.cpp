#include <algorithm>
#include <cstddef>
#include <limits>

#include "rpc/server_contract.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_exec.h"
#include "rpc/server_route_executor.h"
#include "rpc/server_route_image.h"
#include "rpc/server_route_port_tunnel.h"
#include "rpc/server_route_transfer.h"

namespace {

const unsigned long STREAMING_TRANSFER_BODY_IDLE_TIMEOUT_MS = 300000UL;

enum RouteExecutionMode {
    ROUTE_EXECUTION_BUFFERED,
    ROUTE_EXECUTION_STREAMING_IMPORT,
    ROUTE_EXECUTION_STREAMING_EXPORT,
    ROUTE_EXECUTION_UPGRADE,
};

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

HttpResponse route_transfer_path_info(
    const ServerRouteContext& context,
    const HttpRequest& request
) {
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
    {server_contract::ROUTE_TRANSFER_EXPORT,
     &route_transfer_export,
     ROUTE_EXECUTION_STREAMING_EXPORT},
    {server_contract::ROUTE_TRANSFER_PATH_INFO, &route_transfer_path_info, ROUTE_EXECUTION_BUFFERED
    },
    {server_contract::ROUTE_TRANSFER_IMPORT,
     &route_transfer_import,
     ROUTE_EXECUTION_STREAMING_IMPORT},
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

RouteExecutionMode route_execution_mode(const HttpRequest& request) {
    const RouteDispatchEntry* entry =
        find_route_entry(server_contract::route_id_for_path(request.path));
    return entry == nullptr ? ROUTE_EXECUTION_BUFFERED : entry->mode;
}

RpcRouteExecution buffered_execution(HttpResponse response) {
    RpcRouteExecution execution;
    execution.kind = RPC_ROUTE_EXECUTION_BUFFERED_RESPONSE;
    execution.response = response;
    return execution;
}

RpcRouteExecution streaming_import_execution(HttpResponse response, bool close_after_response) {
    RpcRouteExecution execution;
    execution.kind = RPC_ROUTE_EXECUTION_STREAMING_IMPORT_RESPONSE;
    execution.response = response;
    execution.close_after_response = close_after_response;
    return execution;
}

RpcRouteExecution streaming_export_execution(
    HttpResponse response,
    const StreamingTransferExport& transfer
) {
    RpcRouteExecution execution;
    execution.kind = RPC_ROUTE_EXECUTION_STREAMING_RESPONSE;
    execution.response = response;
    execution.write_streaming_response = [transfer](HttpChunkedResponseWriter* chunks) {
        run_streaming_transfer_export(transfer, chunks);
    };
    return execution;
}

RpcRouteExecution upgrade_execution(const PortTunnelUpgradeRoute& upgrade) {
    RpcRouteExecution execution;
    execution.kind = RPC_ROUTE_EXECUTION_UPGRADE_HANDLER;
    execution.response.status = 101;
    execution.close_after_response = true;
    execution.upgrade_token = upgrade.upgrade_token;
    execution.upgrade_headers = upgrade.response_headers;
    execution.run_upgrade = [upgrade](SOCKET client) {
        run_port_tunnel_route_upgrade(upgrade, client);
    };
    return execution;
}

} // namespace

RpcRouteBodyPolicy rpc_route_body_policy(
    const HttpRequest& request,
    std::size_t default_max_body_bytes
) {
    RpcRouteBodyPolicy policy;
    policy.max_body_bytes = default_max_body_bytes;
    if (route_execution_mode(request) == ROUTE_EXECUTION_STREAMING_IMPORT) {
        policy.handling = RPC_ROUTE_BODY_STREAMING;
        policy.max_body_bytes = std::numeric_limits<std::size_t>::max();
        policy.min_idle_timeout_ms = STREAMING_TRANSFER_BODY_IDLE_TIMEOUT_MS;
    }
    return policy;
}

HttpResponse execute_buffered_rpc_route(
    const ServerRouteContext& context,
    const HttpRequest& request
) {
    HttpResponse response;
    response.status = 200;
    if (reject_before_route(context.gate, request, &response)) {
        write_request_id_header(response, request);
        return response;
    }

    const RouteDispatchEntry* entry =
        find_route_entry(server_contract::route_id_for_path(request.path));
    if (entry != nullptr && entry->handler != nullptr) {
        response = entry->handler(context, request);
    } else {
        response = make_rpc_error_response(404, "not_found", "unknown endpoint");
    }

    write_request_id_header(response, request);
    return response;
}

RpcRouteExecution execute_rpc_route(
    const ServerRouteContext& routes,
    const PortTunnelRouteContext& port_tunnel,
    const HttpRequest& request,
    HttpRequestBodyStream* body
) {
    const RouteExecutionMode mode = route_execution_mode(request);
    if (mode == ROUTE_EXECUTION_STREAMING_EXPORT) {
        StreamingTransferExport transfer;
        HttpResponse response =
            prepare_streaming_transfer_export(routes.transfer, request, body, &transfer);
        write_request_id_header(response, request);
        if (response.status != 200) {
            return buffered_execution(response);
        }
        return streaming_export_execution(response, transfer);
    }
    if (mode == ROUTE_EXECUTION_UPGRADE) {
        PortTunnelUpgradeRoute upgrade(port_tunnel);
        HttpResponse response = prepare_port_tunnel_route_upgrade(port_tunnel, request, &upgrade);
        write_request_id_header(response, request);
        if (response.status != 101) {
            return buffered_execution(response);
        }
        return upgrade_execution(upgrade);
    }
    if (mode == ROUTE_EXECUTION_STREAMING_IMPORT) {
        HttpResponse response = handle_streaming_transfer_import(routes.transfer, request, body);
        write_request_id_header(response, request);
        return streaming_import_execution(response, !body->fully_consumed());
    }

    HttpRequest buffered_request = request;
    buffered_request.body = read_request_body_to_string(body);
    return buffered_execution(execute_buffered_rpc_route(routes, buffered_request));
}
