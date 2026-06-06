#pragma once

#include <atomic>

#include "core/config.h"
#include "runtime/app_context.h"

struct AppState {
    // AppState is owned by ServerRuntime and shared by route handlers only for
    // the lifetime of a connection worker. Handlers may use these grouped
    // owners, but they do not own the runtime threads, listener socket, or
    // daemon shutdown sequence.
    DaemonConfig config;
    AppMetadata metadata;
    AppSandboxState sandbox;
    AppServices services;
    AppShutdownState shutdown;
};

ServerRouteContext make_server_route_context(AppState& state);
PortTunnelRouteContext make_port_tunnel_route_context(AppState& state);
HttpConnectionContext make_http_connection_context(AppState& state);
int run_server(const DaemonConfig& config);
