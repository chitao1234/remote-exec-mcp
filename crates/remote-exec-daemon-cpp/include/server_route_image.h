#pragma once

#include "http/http_helpers.h"
#include "runtime/server.h"

HttpResponse handle_image_read(AppState& state, const HttpRequest& request);
