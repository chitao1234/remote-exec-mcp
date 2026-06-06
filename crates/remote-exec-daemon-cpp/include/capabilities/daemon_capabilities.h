#pragma once

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
