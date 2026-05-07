#pragma once

#include <memory>
#include <string>

#include "config.h"
#include "filesystem_sandbox.h"
#include "session_store.h"
#include "server_transport.h"

class PortTunnelService;

struct AppState {
    DaemonConfig config;
    std::string daemon_instance_id;
    std::string hostname;
    std::string default_shell;
    bool sandbox_enabled = false;
    CompiledFilesystemSandbox sandbox;
    SessionStore sessions;
    std::shared_ptr<PortTunnelService> port_tunnel_service;
};

void handle_client(AppState& state, UniqueSocket client);
int run_server(const DaemonConfig& config);
