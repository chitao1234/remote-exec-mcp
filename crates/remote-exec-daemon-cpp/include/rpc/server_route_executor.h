#pragma once

#include <cstddef>
#include <functional>
#include <map>
#include <memory>
#include <string>

#include "http/http_helpers.h"
#include "http/server_transport.h"
#include "runtime/route_context.h"

enum RpcRouteBodyHandling {
    RPC_ROUTE_BODY_BUFFERED,
    RPC_ROUTE_BODY_STREAMING,
};

struct RpcRouteBodyPolicy {
    RpcRouteBodyPolicy()
        : handling(RPC_ROUTE_BODY_BUFFERED), max_body_bytes(0U), min_idle_timeout_ms(0UL) {}

    RpcRouteBodyHandling handling;
    std::size_t max_body_bytes;
    unsigned long min_idle_timeout_ms;
};

enum RpcRouteExecutionKind {
    RPC_ROUTE_EXECUTION_BUFFERED_RESPONSE,
    RPC_ROUTE_EXECUTION_STREAMING_RESPONSE,
    RPC_ROUTE_EXECUTION_STREAMING_IMPORT_RESPONSE,
    RPC_ROUTE_EXECUTION_UPGRADE_HANDLER,
};

typedef std::function<void(HttpChunkedResponseWriter*)> RpcStreamingResponseWriter;
typedef std::function<void(const std::shared_ptr<ConnectionTransport>&)> RpcUpgradeHandler;

struct RpcRouteExecution {
    RpcRouteExecution()
        : kind(RPC_ROUTE_EXECUTION_BUFFERED_RESPONSE), response(), close_after_response(false),
          write_streaming_response(), upgrade_token(), upgrade_headers(), run_upgrade() {}

    RpcRouteExecutionKind kind;
    HttpResponse response;
    bool close_after_response;
    RpcStreamingResponseWriter write_streaming_response;
    std::string upgrade_token;
    std::map<std::string, std::string> upgrade_headers;
    RpcUpgradeHandler run_upgrade;
};

RpcRouteBodyPolicy rpc_route_body_policy(
    const HttpRequest& request,
    std::size_t default_max_body_bytes
);
RpcRouteExecution execute_rpc_route(
    const ServerRouteContext& routes,
    const PortTunnelRouteContext& port_tunnel,
    const HttpRequest& request,
    HttpRequestBodyStream* body
);
HttpResponse execute_buffered_rpc_route(
    const ServerRouteContext& context,
    const HttpRequest& request
);
