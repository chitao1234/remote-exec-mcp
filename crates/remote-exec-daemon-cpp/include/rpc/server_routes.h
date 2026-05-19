#pragma once

#include "http/http_helpers.h"
#include "runtime/server.h"

HttpResponse route_request(AppState& state, const HttpRequest& request);
