#include "rpc/server_route_port_tunnel.h"

#include "core/text_utils.h"
#include "port_forward/port_tunnel.h"
#include "rpc/server_contract.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"

namespace {

std::string header_token_lower(const HttpRequest& request, const std::string& name) {
    return lowercase_ascii(request.header(name));
}

bool connection_header_has_upgrade(const HttpRequest& request) {
    const std::string value = header_token_lower(request, "connection");
    if (value.empty()) {
        return false;
    }
    std::size_t offset = 0;
    while (offset < value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset)
        );
        if (token == "upgrade") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }
    return false;
}

} // namespace

HttpResponse prepare_port_tunnel_route_upgrade(
    const PortTunnelRouteContext& context,
    const HttpRequest& request,
    PortTunnelUpgradeRoute* upgrade
) {
    HttpResponse response;
    if (!context.gate.http_auth_bearer_token.empty()
        && !request_has_bearer_auth(request, context.gate.http_auth_bearer_token)) {
        write_bearer_auth_challenge(response);
        return response;
    }
    if (request.method != "POST"
        || request.path != server_contract::route_path(server_contract::ROUTE_PORT_TUNNEL)
        || !connection_header_has_upgrade(request)
        || header_token_lower(request, "upgrade") != server_contract::PORT_TUNNEL_UPGRADE_TOKEN
        || request.header(server_contract::PORT_TUNNEL_VERSION_HEADER)
               != server_contract::PORT_TUNNEL_VERSION_VALUE) {
        write_rpc_error(response, 400, "bad_request", "invalid port tunnel upgrade request");
        return response;
    }

    upgrade->upgrade_token = server_contract::PORT_TUNNEL_UPGRADE_TOKEN;
    upgrade->response_headers[request_id_header_name()] = request_id_for_request(request);

    response.status = 101;
    return response;
}

void run_port_tunnel_route_upgrade(
    const PortTunnelUpgradeRoute& upgrade,
    const std::shared_ptr<ConnectionTransport>& client
) {
    run_port_tunnel_connection(client, upgrade.context.service, upgrade.context.limits);
}
