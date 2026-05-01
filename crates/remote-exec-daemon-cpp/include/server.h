#pragma once

#include <string>

#include "config.h"
#include "port_forward.h"
#include "session_store.h"
#include "server_transport.h"

struct AppState {
    DaemonConfig config;
    std::string daemon_instance_id;
    std::string hostname;
    std::string default_shell;
    SessionStore sessions;
    PortForwardStore port_forwards;
};

void handle_client_once(AppState& state, UniqueSocket client);
int run_server(const DaemonConfig& config);
