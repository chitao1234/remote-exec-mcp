#pragma once

#include <string>

#include "rpc/exec_request_utils.h"
#include "policy/filesystem_sandbox.h"
#include "http/http_helpers.h"
#include "patch_engine.h"
#include "runtime/server.h"

bool reject_before_route(const AppState& state, const HttpRequest& request, HttpResponse* response);
std::string resolve_workdir(const AppState& state, const Json& body);
std::string resolve_authorized_workdir(const AppState& state, const Json& body, SandboxAccess access);
std::string resolve_input_path(const AppState& state, const Json& body, const std::string& key);
std::string
resolve_authorized_input_path(const AppState& state, const Json& body, const std::string& key, SandboxAccess access);
void authorize_sandbox_path(const AppState& state, SandboxAccess access, const std::string& path);
PatchPathAuthorizer make_patch_path_authorizer(const AppState& state);
