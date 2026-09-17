#pragma once

struct DaemonConfig;

struct DaemonCapabilities {
    DaemonCapabilities();

    bool supports_exec;
    bool supports_apply_patch;
    bool supports_pty;
    bool supports_image_read;
    bool supports_transfer_compression;
    bool supports_port_forward;
    unsigned int port_forward_protocol_version;
    unsigned int transfer_stream_protocol_version;
    // Always advertised: the broker decides whether to expose remote_read,
    // remote_write, and remote_edit through its own [tools.file] config,
    // exactly as it does for the Rust daemon.
    unsigned int file_tool_protocol_version;
};

DaemonCapabilities detect_daemon_capabilities(const DaemonConfig& config);
