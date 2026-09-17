#pragma once

#include "http/http_helpers.h"
#include "runtime/route_context.h"

HttpResponse handle_file_read(const FileRouteContext& context, const HttpRequest& request);
HttpResponse handle_file_write(const FileRouteContext& context, const HttpRequest& request);
HttpResponse handle_file_edit(const FileRouteContext& context, const HttpRequest& request);
