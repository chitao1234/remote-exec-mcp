#pragma once

#include <memory>

#include "port_tunnel_session_state.h"

void close_session_attachment(const std::shared_ptr<PortTunnelSessionAttachment>& attachment);
void finish_terminal_session_teardown(const PortTunnelSessionTeardown& state);
void close_retained_resource(const PortTunnelRetainedResource& resource);
