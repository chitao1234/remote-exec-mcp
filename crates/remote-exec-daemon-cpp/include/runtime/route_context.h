#pragma once

#include <atomic>
#include <cstddef>
#include <memory>
#include <string>

#include "capabilities/daemon_capabilities.h"
#include "core/config.h"
#include "exec/session_store.h"
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
        const CompiledFilesystemSandbox* active_sandbox_value
    )
        : default_workdir(default_workdir_value), active_sandbox(active_sandbox_value) {}

    const std::string& default_workdir;
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
        : target(target_value), daemon_instance_id(daemon_instance_id_value), hostname(hostname_value),
          capabilities(capabilities_value) {}

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

struct ExecRouteContext {
    ExecRouteContext(
        const ExecRequestContext& request_value,
        const std::string& target_value,
        SessionStore& sessions_value,
        const YieldTimeConfig& yield_time_value,
        unsigned long max_open_sessions_value,
        const std::string& daemon_instance_id_value
    )
        : request(request_value), target(target_value), sessions(sessions_value), yield_time(yield_time_value),
          max_open_sessions(max_open_sessions_value), daemon_instance_id(daemon_instance_id_value) {}

    ExecRequestContext request;
    const std::string& target;
    SessionStore& sessions;
    const YieldTimeConfig& yield_time;
    unsigned long max_open_sessions;
    const std::string& daemon_instance_id;
};

struct PatchRouteContext {
    PatchRouteContext(const PathResolutionContext& paths_value, const std::string& daemon_instance_id_value)
        : paths(paths_value), daemon_instance_id(daemon_instance_id_value) {}

    PathResolutionContext paths;
    const std::string& daemon_instance_id;
};

struct ImageRouteContext {
    explicit ImageRouteContext(const PathResolutionContext& paths_value) : paths(paths_value) {}

    PathResolutionContext paths;
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

struct PortTunnelRouteContext {
    PortTunnelRouteContext(
        const HttpGateContext& gate_value,
        const PortForwardLimitConfig& limits_value,
        std::shared_ptr<PortTunnelService>& service_value
    )
        : gate(gate_value), limits(limits_value), service(service_value) {}

    HttpGateContext gate;
    const PortForwardLimitConfig& limits;
    std::shared_ptr<PortTunnelService>& service;
};

struct ServerRouteContext {
    ServerRouteContext(
        const HttpGateContext& gate_value,
        const HealthRouteContext& health_value,
        const TargetInfoRouteContext& target_info_value,
        const ExecRouteContext& exec_value,
        const PatchRouteContext& patch_value,
        const ImageRouteContext& image_value,
        const TransferRouteContext& transfer_value
    )
        : gate(gate_value), health(health_value), target_info(target_info_value), exec(exec_value), patch(patch_value),
          image(image_value), transfer(transfer_value) {}

    HttpGateContext gate;
    HealthRouteContext health;
    TargetInfoRouteContext target_info;
    ExecRouteContext exec;
    PatchRouteContext patch;
    ImageRouteContext image;
    TransferRouteContext transfer;
};

struct HttpConnectionContext {
    HttpConnectionContext(
        const ServerRouteContext& routes_value,
        const PortTunnelRouteContext& port_tunnel_value,
        std::size_t max_request_header_bytes_value,
        std::size_t max_request_body_bytes_value,
        unsigned long http_connection_idle_timeout_ms_value,
        const std::atomic<bool>& shutdown_requested_value
    )
        : routes(routes_value), port_tunnel(port_tunnel_value),
          max_request_header_bytes(max_request_header_bytes_value),
          max_request_body_bytes(max_request_body_bytes_value),
          http_connection_idle_timeout_ms(http_connection_idle_timeout_ms_value),
          shutdown_requested(shutdown_requested_value) {}

    ServerRouteContext routes;
    PortTunnelRouteContext port_tunnel;
    std::size_t max_request_header_bytes;
    std::size_t max_request_body_bytes;
    unsigned long http_connection_idle_timeout_ms;
    const std::atomic<bool>& shutdown_requested;
};
