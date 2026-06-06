#include "runtime/server.h"

ServerRouteContext make_server_route_context(AppState& state) {
    ServerRouteContext context;
    context.gate.http_auth_bearer_token = &state.config.http_auth_bearer_token;
    context.health.daemon_instance_id = &state.metadata.daemon_instance_id;
    context.target_info.target = &state.config.target;
    context.target_info.metadata = &state.metadata;
    context.exec.request.paths.default_workdir = &state.config.default_workdir;
    context.exec.request.paths.sandbox = &state.sandbox;
    context.exec.request.capabilities = &state.metadata.capabilities;
    context.exec.request.default_shell = &state.metadata.default_shell;
    context.exec.request.allow_login_shell = state.config.allow_login_shell;
    context.exec.target = &state.config.target;
    context.exec.sessions = &state.services.sessions;
    context.exec.yield_time = &state.config.yield_time;
    context.exec.max_open_sessions = state.config.max_open_sessions;
    context.exec.daemon_instance_id = &state.metadata.daemon_instance_id;
    context.patch.paths.default_workdir = &state.config.default_workdir;
    context.patch.paths.sandbox = &state.sandbox;
    context.patch.daemon_instance_id = &state.metadata.daemon_instance_id;
    context.image.paths.default_workdir = &state.config.default_workdir;
    context.image.paths.sandbox = &state.sandbox;
    context.transfer.gate = context.gate;
    context.transfer.paths.default_workdir = &state.config.default_workdir;
    context.transfer.paths.sandbox = &state.sandbox;
    context.transfer.limits = &state.config.transfer_limits;
    return context;
}

PortTunnelRouteContext make_port_tunnel_route_context(AppState& state) {
    PortTunnelRouteContext context;
    context.gate.http_auth_bearer_token = &state.config.http_auth_bearer_token;
    context.limits = &state.config.port_forward_limits;
    context.service = &state.services.port_tunnel;
    return context;
}

HttpConnectionContext make_http_connection_context(AppState& state) {
    HttpConnectionContext context;
    context.routes = make_server_route_context(state);
    context.port_tunnel = make_port_tunnel_route_context(state);
    context.max_request_header_bytes = state.config.max_request_header_bytes;
    context.max_request_body_bytes = state.config.max_request_body_bytes;
    context.http_connection_idle_timeout_ms = state.config.http_connection_idle_timeout_ms;
    context.shutdown_requested = &state.shutdown.requested;
    return context;
}
