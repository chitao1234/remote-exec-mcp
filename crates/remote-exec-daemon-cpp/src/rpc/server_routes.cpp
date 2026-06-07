#include "rpc/server_routes.h"

#include "rpc/server_route_executor.h"

HttpResponse route_request(const ServerRouteContext& context, const HttpRequest& request) {
    return execute_buffered_rpc_route(context, request);
}
