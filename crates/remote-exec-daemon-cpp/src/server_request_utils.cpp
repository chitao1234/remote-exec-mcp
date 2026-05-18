#include <string>

#include "path_policy.h"
#include "server_request_utils.h"

namespace {

const CompiledFilesystemSandbox* active_sandbox(const AppState& state) {
    return state.sandbox_enabled ? &state.sandbox : nullptr;
}

std::string resolve_path_from_base(const std::string& base, const std::string& raw) {
    const PathPolicy policy = host_path_policy();
    if (is_absolute_for_policy(policy, raw)) {
        return normalize_for_system(policy, raw);
    }
    return join_for_policy(policy, base, raw);
}

} // namespace

bool reject_before_route(const AppState& state, const HttpRequest& request, HttpResponse* response) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        write_bearer_auth_challenge(*response);
        return true;
    }

    if (request.method != "POST") {
        write_rpc_error(*response, 405, "method_not_allowed", "only POST is supported");
        return true;
    }

    return false;
}

std::string resolve_workdir(const AppState& state, const Json& body) {
    const std::string raw = body.value("workdir", state.config.default_workdir);
    if (raw.empty()) {
        return state.config.default_workdir;
    }

    return resolve_path_from_base(state.config.default_workdir, raw);
}

std::string resolve_authorized_workdir(const AppState& state, const Json& body, SandboxAccess access) {
    const std::string path = resolve_workdir(state, body);
    authorize_sandbox_path(state, access, path);
    return path;
}

std::string resolve_input_path(const AppState& state, const Json& body, const std::string& key) {
    const std::string raw = body.at(key).get<std::string>();
    return resolve_path_from_base(resolve_workdir(state, body), raw);
}

std::string
resolve_authorized_input_path(const AppState& state, const Json& body, const std::string& key, SandboxAccess access) {
    const std::string path = resolve_input_path(state, body, key);
    authorize_sandbox_path(state, access, path);
    return path;
}

void authorize_sandbox_path(const AppState& state, SandboxAccess access, const std::string& path) {
    authorize_path(active_sandbox(state), access, path);
}

PatchPathAuthorizer make_patch_path_authorizer(const AppState& state) {
    if (!state.sandbox_enabled) {
        return PatchPathAuthorizer();
    }
    return [&state](const std::string& path) { authorize_sandbox_path(state, SANDBOX_WRITE, path); };
}
