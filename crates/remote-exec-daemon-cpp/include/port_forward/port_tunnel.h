#pragma once

#include <memory>

#include "core/config.h"
#include "platform/socket.h"

class PortTunnelService;

void run_port_tunnel_connection(SOCKET client,
                                std::shared_ptr<PortTunnelService>& service,
                                const PortForwardLimitConfig& limits);
std::shared_ptr<PortTunnelService>
create_port_tunnel_service(const PortForwardLimitConfig& limits = PortForwardLimitConfig());

#ifdef REMOTE_EXEC_CPP_TESTING
void set_forced_tcp_read_thread_failures(unsigned long count);
void set_forced_tcp_write_thread_start_delay_ms(unsigned long delay_ms);
#endif
