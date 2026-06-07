#pragma once

#include "runtime/app_state.h"
#include "runtime/route_context.h"

ServerRouteContext make_server_route_context(
    const DaemonConfig& config,
    const AppMetadata& metadata,
    const AppSandboxState& sandbox,
    AppServices& services
);
PortTunnelRouteContext make_port_tunnel_route_context(
    const DaemonConfig& config,
    AppServices& services
);
HttpConnectionContext make_http_connection_context(
    const DaemonConfig& config,
    const AppMetadata& metadata,
    const AppSandboxState& sandbox,
    AppServices& services,
    const AppShutdownState& shutdown
);
