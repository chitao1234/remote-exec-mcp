#pragma once

#include <memory>

#include "config.h"
#include "http_helpers.h"
#include "server.h"

class PortTunnelService;

bool is_port_tunnel_upgrade_request(const HttpRequest& request);
int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request);
std::shared_ptr<PortTunnelService>
create_port_tunnel_service(const PortForwardLimitConfig& limits = PortForwardLimitConfig());

#ifdef REMOTE_EXEC_CPP_TESTING
void set_forced_tcp_read_thread_failures(unsigned long count);
#endif
