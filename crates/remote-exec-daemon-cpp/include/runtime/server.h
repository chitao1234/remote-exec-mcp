#pragma once

#include <memory>
#include <string>

#include "capabilities/daemon_capabilities.h"
#include "core/config.h"
#include "policy/filesystem_sandbox.h"
#include "platform/socket.h"
#include "exec/session_store.h"

class PortTunnelService;

struct AppState {
    // AppState is owned by ServerRuntime and shared by route handlers only for
    // the lifetime of a connection worker. Route handlers may create or close
    // subsystem resources through these owners, but they do not own the runtime
    // threads, listener socket, or daemon shutdown sequence.
    DaemonConfig config;
    std::string daemon_instance_id;
    std::string hostname;
    std::string default_shell;
    DaemonCapabilities capabilities;
    bool sandbox_enabled = false;
    CompiledFilesystemSandbox sandbox;
    SessionStore sessions;
    std::shared_ptr<PortTunnelService> port_tunnel_service;
};

void handle_client(AppState& state, UniqueSocket client);
int run_server(const DaemonConfig& config);
