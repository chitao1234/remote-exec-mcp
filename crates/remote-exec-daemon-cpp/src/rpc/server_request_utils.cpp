#include <string>

#include "policy/path_policy.h"
#include "rpc/server_request_utils.h"

namespace {

const CompiledFilesystemSandbox* active_sandbox(const PathResolutionContext& context) {
    return context.sandbox->active();
}

std::string resolve_path_from_base(const std::string& base, const std::string& raw) {
    const PathPolicy policy = host_path_policy();
    if (is_absolute_for_policy(policy, raw)) {
        return normalize_for_system(policy, raw);
    }
    return join_for_policy(policy, base, raw);
}

} // namespace

bool reject_before_route(const HttpGateContext& context, const HttpRequest& request, HttpResponse* response) {
    if (!context.http_auth_bearer_token->empty() &&
        !request_has_bearer_auth(request, *context.http_auth_bearer_token)) {
        write_bearer_auth_challenge(*response);
        return true;
    }

    if (request.method != "POST") {
        write_rpc_error(*response, 405, "method_not_allowed", "only POST is supported");
        return true;
    }

    return false;
}

std::string resolve_workdir(const PathResolutionContext& context, const Json& body) {
    const std::string raw = body.value("workdir", *context.default_workdir);
    if (raw.empty()) {
        return *context.default_workdir;
    }

    return resolve_path_from_base(*context.default_workdir, raw);
}

std::string resolve_authorized_workdir(const PathResolutionContext& context, const Json& body, SandboxAccess access) {
    const std::string path = resolve_workdir(context, body);
    authorize_sandbox_path(context, access, path);
    return path;
}

std::string resolve_input_path(const PathResolutionContext& context, const Json& body, const std::string& key) {
    const std::string raw = body.at(key).get<std::string>();
    return resolve_path_from_base(resolve_workdir(context, body), raw);
}

std::string resolve_authorized_input_path(const PathResolutionContext& context,
                                          const Json& body,
                                          const std::string& key,
                                          SandboxAccess access) {
    const std::string path = resolve_input_path(context, body, key);
    authorize_sandbox_path(context, access, path);
    return path;
}

void authorize_sandbox_path(const PathResolutionContext& context, SandboxAccess access, const std::string& path) {
    authorize_path(active_sandbox(context), access, path);
}

PatchPathAuthorizer make_patch_path_authorizer(const PathResolutionContext& context) {
    if (!context.sandbox->enabled) {
        return PatchPathAuthorizer();
    }
    return [context](const std::string& path) { authorize_sandbox_path(context, SANDBOX_WRITE, path); };
}
