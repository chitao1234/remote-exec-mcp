#pragma once

#include "http/http_helpers.h"
#include "runtime/server.h"

enum RouteExecutionMode {
    ROUTE_EXECUTION_BUFFERED,
    ROUTE_EXECUTION_STREAMING_IMPORT,
    ROUTE_EXECUTION_STREAMING_EXPORT,
    ROUTE_EXECUTION_UPGRADE,
};

RouteExecutionMode route_execution_mode(const HttpRequest& request);
HttpResponse route_request(AppState& state, const HttpRequest& request);
