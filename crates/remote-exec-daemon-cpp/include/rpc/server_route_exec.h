#pragma once

#include "http/http_helpers.h"
#include "runtime/app_context.h"

HttpResponse handle_exec_start(const ExecRouteContext& context, const HttpRequest& request);
HttpResponse handle_exec_write(const ExecRouteContext& context, const HttpRequest& request);
