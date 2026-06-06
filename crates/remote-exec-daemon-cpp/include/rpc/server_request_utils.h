#pragma once

#include <string>

#include "http/http_helpers.h"
#include "patch/patch_engine.h"
#include "policy/filesystem_sandbox.h"
#include "rpc/exec_request_utils.h"
#include "runtime/app_context.h"

bool reject_before_route(const HttpGateContext& context, const HttpRequest& request, HttpResponse* response);
std::string resolve_workdir(const PathResolutionContext& context, const Json& body);
std::string resolve_authorized_workdir(const PathResolutionContext& context, const Json& body, SandboxAccess access);
std::string resolve_input_path(const PathResolutionContext& context, const Json& body, const std::string& key);
std::string resolve_authorized_input_path(const PathResolutionContext& context,
                                          const Json& body,
                                          const std::string& key,
                                          SandboxAccess access);
void authorize_sandbox_path(const PathResolutionContext& context, SandboxAccess access, const std::string& path);
PatchPathAuthorizer make_patch_path_authorizer(const PathResolutionContext& context);
