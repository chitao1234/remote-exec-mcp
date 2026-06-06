#pragma once

#include <functional>
#include <string>

#include "core/logging.h"
#include "http/http_helpers.h"
#include "runtime/app_context.h"

enum class ExecRouteKind {
    Start,
    Write,
};

typedef std::function<void(HttpResponse&)> RpcRouteBody;

HttpResponse handle_transfer_rpc_route(const std::string& route_name, const RpcRouteBody& body);
HttpResponse handle_image_rpc_route(const std::string& route_name, const RpcRouteBody& body);
HttpResponse handle_exec_rpc_route(const std::string& route_name, ExecRouteKind kind, const RpcRouteBody& body);
HttpResponse handle_patch_rpc_route(const RpcRouteBody& body);
HttpResponse make_rpc_error_response(int status, const std::string& code, const std::string& message);
HttpResponse handle_health(const HealthRouteContext& context);
HttpResponse handle_target_info(const TargetInfoRouteContext& context);
HttpResponse handle_patch_apply(const PatchRouteContext& context, const HttpRequest& request);
LogLevel level_for_status(int status);
