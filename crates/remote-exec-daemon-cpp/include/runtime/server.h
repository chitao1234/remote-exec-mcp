#pragma once

#include <atomic>
#include <memory>
#include <string>

#include "capabilities/daemon_capabilities.h"
#include "core/config.h"
#include "exec/session_store.h"
#include "platform/socket.h"
#include "policy/filesystem_sandbox.h"

class PortTunnelService;

struct AppMetadata {
    std::string daemon_instance_id;
    std::string hostname;
    std::string default_shell;
    DaemonCapabilities capabilities;
};

struct AppSandboxState {
    bool enabled = false;
    CompiledFilesystemSandbox compiled;

    const CompiledFilesystemSandbox* active() const { return enabled ? &compiled : nullptr; }
};

struct AppServices {
    SessionStore sessions;
    std::shared_ptr<PortTunnelService> port_tunnel;
};

struct AppShutdownState {
    std::atomic<bool> requested{false};
};

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

void handle_client(AppState& state, UniqueSocket client);
int run_server(const DaemonConfig& config);
