#pragma once

#include <string>

#include "http/http_helpers.h"
#include "core/logging.h"
#include "runtime/server.h"

HttpResponse make_rpc_error_response(int status, const std::string& code, const std::string& message);
HttpResponse handle_health(const AppState& state);
HttpResponse handle_target_info(const AppState& state);
HttpResponse handle_patch_apply(AppState& state, const HttpRequest& request);
LogLevel level_for_status(int status);
