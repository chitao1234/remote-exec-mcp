#pragma once

#include "http_helpers.h"
#include "server.h"

HttpResponse handle_exec_start(AppState& state, const HttpRequest& request);
HttpResponse handle_exec_write(AppState& state, const HttpRequest& request);
