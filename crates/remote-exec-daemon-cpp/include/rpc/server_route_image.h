#pragma once

#include "http/http_helpers.h"
#include "runtime/app_context.h"

HttpResponse handle_image_read(const ImageRouteContext& context, const HttpRequest& request);
