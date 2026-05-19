#pragma once

#include "http/http_helpers.h"
#include "runtime/server.h"

HttpResponse handle_exec_start(AppState& state, const HttpRequest& request);
HttpResponse handle_exec_write(AppState& state, const HttpRequest& request);
