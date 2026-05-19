#pragma once

#include "http/http_helpers.h"
#include "runtime/server.h"

HttpResponse handle_transfer_export(AppState& state, const HttpRequest& request);
HttpResponse handle_transfer_path_info(AppState& state, const HttpRequest& request);
HttpResponse handle_transfer_import(AppState& state, const HttpRequest& request);
