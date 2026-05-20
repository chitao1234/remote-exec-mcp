#pragma once

#include "json.hpp"

using Json = nlohmann::json;

struct DaemonCapabilities {
    DaemonCapabilities();

    bool supports_pty;
    bool supports_image_read;
    bool supports_transfer_compression;
    bool supports_port_forward;
    unsigned int port_forward_protocol_version;
    unsigned int transfer_stream_protocol_version;
};

DaemonCapabilities detect_daemon_capabilities();
void write_daemon_capabilities(Json* target, const DaemonCapabilities& capabilities);
