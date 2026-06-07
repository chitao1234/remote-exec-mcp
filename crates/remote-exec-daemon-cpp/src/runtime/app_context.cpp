#include "runtime/app_context.h"

ServerRouteContext make_server_route_context(
    const DaemonConfig& config,
    const AppMetadata& metadata,
    const AppSandboxState& sandbox,
    AppServices& services
) {
    const HttpGateContext gate(config.http_auth_bearer_token);
    const PathResolutionContext paths(config.default_workdir, sandbox.active());
    const HealthRouteContext health(metadata.daemon_instance_id);
    const TargetInfoRouteContext target_info(
        config.target,
        metadata.daemon_instance_id,
        metadata.hostname,
        metadata.capabilities
    );
    const ExecRequestContext exec_request(
        paths,
        metadata.capabilities,
        metadata.default_shell,
        config.allow_login_shell
    );
    const ExecRouteContext exec(
        exec_request,
        config.target,
        services.sessions,
        config.yield_time,
        config.max_open_sessions,
        metadata.daemon_instance_id
    );
    const PatchRouteContext patch(paths, metadata.daemon_instance_id);
    const ImageRouteContext image(paths);
    const TransferRouteContext transfer(gate, paths, config.transfer_limits);
    return ServerRouteContext(gate, health, target_info, exec, patch, image, transfer);
}

PortTunnelRouteContext make_port_tunnel_route_context(
    const DaemonConfig& config,
    AppServices& services
) {
    return PortTunnelRouteContext(
        HttpGateContext(config.http_auth_bearer_token),
        config.port_forward_limits,
        services.port_tunnel
    );
}

HttpConnectionContext make_http_connection_context(
    const DaemonConfig& config,
    const AppMetadata& metadata,
    const AppSandboxState& sandbox,
    AppServices& services,
    const AppShutdownState& shutdown
) {
    return HttpConnectionContext(
        make_server_route_context(config, metadata, sandbox, services),
        make_port_tunnel_route_context(config, services),
        config.max_request_header_bytes,
        config.max_request_body_bytes,
        config.http_connection_idle_timeout_ms,
        shutdown.requested
    );
}
