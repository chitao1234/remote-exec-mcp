#pragma once

#include "http_helpers.h"
#include "server.h"

bool is_port_tunnel_upgrade_request(const HttpRequest& request);
int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request);
