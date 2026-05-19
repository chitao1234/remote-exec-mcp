#pragma once

#include "json.hpp"

struct AppState;
using Json = nlohmann::json;

struct DaemonCapabilities {
    bool supports_pty;
    bool supports_image_read;
    bool supports_transfer_compression;
    bool supports_port_forward;
    unsigned int port_forward_protocol_version;
};

DaemonCapabilities detect_daemon_capabilities(const AppState& state);
void write_daemon_capabilities(Json* target, const DaemonCapabilities& capabilities);
