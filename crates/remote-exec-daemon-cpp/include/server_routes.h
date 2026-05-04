#pragma once

#include "server.h"
#include "http_helpers.h"

HttpResponse route_request(AppState& state, const HttpRequest& request);
