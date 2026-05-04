#pragma once

#include "http_helpers.h"
#include "server.h"

HttpResponse handle_image_read(AppState& state, const HttpRequest& request);
