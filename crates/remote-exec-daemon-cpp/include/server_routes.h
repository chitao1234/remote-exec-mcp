#pragma once

#include "http_helpers.h"
#include "server.h"

HttpResponse route_request(AppState& state, const HttpRequest& request);
