#pragma once

#include <atomic>
#include <cstddef>
#include <memory>
#include <string>

#include "capabilities/daemon_capabilities.h"
#include "core/config.h"
#include "patch/patch_engine.h"
#include "policy/file_ops.h"
#include "policy/filesystem_sandbox.h"

class PortTunnelService;

struct HttpGateContext {
    explicit HttpGateContext(const std::string& http_auth_bearer_token_value)
        : http_auth_bearer_token(http_auth_bearer_token_value) {}

    const std::string& http_auth_bearer_token;
};

struct PathResolutionContext {
    PathResolutionContext(
        const std::string& default_workdir_value,
        const std::string& windows_posix_root_value,
        const CompiledFilesystemSandbox* active_sandbox_value
    )
        : default_workdir(default_workdir_value), windows_posix_root(windows_posix_root_value),
          active_sandbox(active_sandbox_value) {}

    const std::string& default_workdir;
    const std::string& windows_posix_root;
    const CompiledFilesystemSandbox* active_sandbox;
};

struct HealthRouteContext {
    explicit HealthRouteContext(const std::string& daemon_instance_id_value)
        : daemon_instance_id(daemon_instance_id_value) {}

    const std::string& daemon_instance_id;
};

struct TargetInfoRouteContext {
    TargetInfoRouteContext(
        const std::string& target_value,
        const std::string& daemon_instance_id_value,
        const std::string& hostname_value,
        const DaemonCapabilities& capabilities_value
    )
        : target(target_value), daemon_instance_id(daemon_instance_id_value),
          hostname(hostname_value), capabilities(capabilities_value) {}

    const std::string& target;
    const std::string& daemon_instance_id;
    const std::string& hostname;
    const DaemonCapabilities& capabilities;
};

struct ExecRequestContext {
    ExecRequestContext(
        const PathResolutionContext& paths_value,
        const DaemonCapabilities& capabilities_value,
        const std::string& default_shell_value,
        bool allow_login_shell_value
    )
        : paths(paths_value), capabilities(capabilities_value), default_shell(default_shell_value),
          allow_login_shell(allow_login_shell_value) {}

    PathResolutionContext paths;
    const DaemonCapabilities& capabilities;
    const std::string& default_shell;
    bool allow_login_shell;
};

struct PatchRouteContext {
    PatchRouteContext(
        const PathResolutionContext& paths_value,
        const std::string& daemon_instance_id_value,
        bool allow_apply_patch_value,
        const PatchApplyOptions& options_value
    )
        : paths(paths_value), daemon_instance_id(daemon_instance_id_value),
          allow_apply_patch(allow_apply_patch_value), options(options_value) {}

    PathResolutionContext paths;
    const std::string& daemon_instance_id;
    bool allow_apply_patch;
    PatchApplyOptions options;
};

struct ImageRouteContext {
    explicit ImageRouteContext(const PathResolutionContext& paths_value) : paths(paths_value) {}

    PathResolutionContext paths;
};

struct FileRouteContext {
    FileRouteContext(
        const PathResolutionContext& paths_value,
        const TextDecodeSettings& text_decode_value
    )
        : paths(paths_value), text_decode(text_decode_value) {}

    PathResolutionContext paths;
    TextDecodeSettings text_decode;
};

struct TransferRouteContext {
    TransferRouteContext(
        const HttpGateContext& gate_value,
        const PathResolutionContext& paths_value,
        const TransferLimitConfig& limits_value
    )
        : gate(gate_value), paths(paths_value), limits(limits_value) {}

    HttpGateContext gate;
    PathResolutionContext paths;
    const TransferLimitConfig& limits;
};
