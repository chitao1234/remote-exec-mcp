#pragma once

#include <atomic>
#include <memory>
#include <string>

#include "capabilities/daemon_capabilities.h"
#include "core/config.h"
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

struct AppShutdownState {
    std::atomic<bool> requested{false};
};
