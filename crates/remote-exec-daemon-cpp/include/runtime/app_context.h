#pragma once

#include <atomic>
#include <cstddef>
#include <memory>
#include <string>

#include "capabilities/daemon_capabilities.h"
#include "core/config.h"
#include "exec/session_store.h"
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

struct HttpGateContext {
    const std::string* http_auth_bearer_token;
};

struct PathResolutionContext {
    const std::string* default_workdir;
    const AppSandboxState* sandbox;
};

struct HealthRouteContext {
    const std::string* daemon_instance_id;
};

struct TargetInfoRouteContext {
    const std::string* target;
    const AppMetadata* metadata;
};

struct ExecRequestContext {
    PathResolutionContext paths;
    const DaemonCapabilities* capabilities;
    const std::string* default_shell;
    bool allow_login_shell;
};

struct ExecRouteContext {
    ExecRequestContext request;
    const std::string* target;
    SessionStore* sessions;
    const YieldTimeConfig* yield_time;
    unsigned long max_open_sessions;
    const std::string* daemon_instance_id;
};

struct PatchRouteContext {
    PathResolutionContext paths;
    const std::string* daemon_instance_id;
};

struct ImageRouteContext {
    PathResolutionContext paths;
};

struct TransferRouteContext {
    HttpGateContext gate;
    PathResolutionContext paths;
    const TransferLimitConfig* limits;
};

struct PortTunnelRouteContext {
    HttpGateContext gate;
    const PortForwardLimitConfig* limits;
    std::shared_ptr<PortTunnelService>* service;
};

struct ServerRouteContext {
    HttpGateContext gate;
    HealthRouteContext health;
    TargetInfoRouteContext target_info;
    ExecRouteContext exec;
    PatchRouteContext patch;
    ImageRouteContext image;
    TransferRouteContext transfer;
};

struct HttpConnectionContext {
    ServerRouteContext routes;
    PortTunnelRouteContext port_tunnel;
    std::size_t max_request_header_bytes;
    std::size_t max_request_body_bytes;
    unsigned long http_connection_idle_timeout_ms;
    const std::atomic<bool>* shutdown_requested;
};
