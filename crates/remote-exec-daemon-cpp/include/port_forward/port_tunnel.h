#pragma once

#include <memory>

#include "core/config.h"
#include "http/http_helpers.h"
#include "platform/socket.h"
#include "runtime/route_context.h"

class PortTunnelService;

int handle_port_tunnel_upgrade(const PortTunnelRouteContext& context, SOCKET client, const HttpRequest& request);
std::shared_ptr<PortTunnelService>
create_port_tunnel_service(const PortForwardLimitConfig& limits = PortForwardLimitConfig());

#ifdef REMOTE_EXEC_CPP_TESTING
void set_forced_tcp_read_thread_failures(unsigned long count);
void set_forced_tcp_write_thread_start_delay_ms(unsigned long delay_ms);
#endif
