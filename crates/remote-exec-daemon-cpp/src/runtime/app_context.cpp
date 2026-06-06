#include "runtime/app_context.h"

ServerRouteContext make_server_route_context(const DaemonConfig& config,
                                             const AppMetadata& metadata,
                                             const AppSandboxState& sandbox,
                                             AppServices& services) {
    ServerRouteContext context;
    context.gate.http_auth_bearer_token = &config.http_auth_bearer_token;
    context.health.daemon_instance_id = &metadata.daemon_instance_id;
    context.target_info.target = &config.target;
    context.target_info.daemon_instance_id = &metadata.daemon_instance_id;
    context.target_info.hostname = &metadata.hostname;
    context.target_info.capabilities = &metadata.capabilities;
    context.exec.request.paths.default_workdir = &config.default_workdir;
    context.exec.request.paths.active_sandbox = sandbox.active();
    context.exec.request.capabilities = &metadata.capabilities;
    context.exec.request.default_shell = &metadata.default_shell;
    context.exec.request.allow_login_shell = config.allow_login_shell;
    context.exec.target = &config.target;
    context.exec.sessions = &services.sessions;
    context.exec.yield_time = &config.yield_time;
    context.exec.max_open_sessions = config.max_open_sessions;
    context.exec.daemon_instance_id = &metadata.daemon_instance_id;
    context.patch.paths.default_workdir = &config.default_workdir;
    context.patch.paths.active_sandbox = sandbox.active();
    context.patch.daemon_instance_id = &metadata.daemon_instance_id;
    context.image.paths.default_workdir = &config.default_workdir;
    context.image.paths.active_sandbox = sandbox.active();
    context.transfer.gate = context.gate;
    context.transfer.paths.default_workdir = &config.default_workdir;
    context.transfer.paths.active_sandbox = sandbox.active();
    context.transfer.limits = &config.transfer_limits;
    return context;
}

PortTunnelRouteContext make_port_tunnel_route_context(const DaemonConfig& config, AppServices& services) {
    PortTunnelRouteContext context;
    context.gate.http_auth_bearer_token = &config.http_auth_bearer_token;
    context.limits = &config.port_forward_limits;
    context.service = &services.port_tunnel;
    return context;
}

HttpConnectionContext make_http_connection_context(const DaemonConfig& config,
                                                   const AppMetadata& metadata,
                                                   const AppSandboxState& sandbox,
                                                   AppServices& services,
                                                   const AppShutdownState& shutdown) {
    HttpConnectionContext context;
    context.routes = make_server_route_context(config, metadata, sandbox, services);
    context.port_tunnel = make_port_tunnel_route_context(config, services);
    context.max_request_header_bytes = config.max_request_header_bytes;
    context.max_request_body_bytes = config.max_request_body_bytes;
    context.http_connection_idle_timeout_ms = config.http_connection_idle_timeout_ms;
    context.shutdown_requested = &shutdown.requested;
    return context;
}
