#pragma once

#include "http_helpers.h"
#include "logging.h"
#include "server.h"

LogLevel level_for_status(int status);
HttpResponse route_request(AppState& state, const HttpRequest& request);
