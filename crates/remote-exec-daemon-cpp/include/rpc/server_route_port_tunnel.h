#pragma once

#include <map>
#include <string>

#include "http/http_helpers.h"
#include "platform/socket.h"
#include "runtime/route_context.h"

struct PortTunnelUpgradeRoute {
    explicit PortTunnelUpgradeRoute(const PortTunnelRouteContext& context_value)
        : context(context_value), upgrade_token(), response_headers() {}

    PortTunnelRouteContext context;
    std::string upgrade_token;
    std::map<std::string, std::string> response_headers;
};

HttpResponse prepare_port_tunnel_route_upgrade(const PortTunnelRouteContext& context,
                                               const HttpRequest& request,
                                               PortTunnelUpgradeRoute* upgrade);
void run_port_tunnel_route_upgrade(const PortTunnelUpgradeRoute& upgrade, SOCKET client);
