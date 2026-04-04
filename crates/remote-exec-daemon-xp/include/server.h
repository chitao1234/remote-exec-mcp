#pragma once

#include <string>

#include "config.h"
#include "session_store.h"

struct AppState {
    DaemonConfig config;
    std::string daemon_instance_id;
    std::string hostname;
    SessionStore sessions;
};

int run_server(const DaemonConfig& config);
