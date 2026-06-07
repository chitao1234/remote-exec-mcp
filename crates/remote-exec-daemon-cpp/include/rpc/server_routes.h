#pragma once

#include "http/http_helpers.h"
#include "runtime/route_context.h"

HttpResponse route_request(const ServerRouteContext& context, const HttpRequest& request);
