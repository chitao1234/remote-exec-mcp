#include "capabilities/daemon_capabilities.h"

#include "core/config.h"
#include "exec/process_session.h"
#include "rpc/server_contract.h"

DaemonCapabilities::DaemonCapabilities()
    : supports_exec(true), supports_apply_patch(true), supports_pty(false),
      supports_image_read(false), supports_transfer_compression(false),
      supports_port_forward(false), port_forward_protocol_version(0U),
      transfer_stream_protocol_version(0U) {
}

DaemonCapabilities detect_daemon_capabilities(const DaemonConfig& config) {
    DaemonCapabilities capabilities;
    capabilities.supports_exec = config.allow_exec;
    capabilities.supports_apply_patch = config.allow_apply_patch;
    capabilities.supports_pty = process_session_supports_pty();
    capabilities.supports_image_read = true;
    capabilities.supports_transfer_compression = false;
    capabilities.supports_port_forward = true;
    capabilities.port_forward_protocol_version = server_contract::PORT_TUNNEL_PROTOCOL_VERSION;
    capabilities.transfer_stream_protocol_version =
        server_contract::TRANSFER_STREAM_PROTOCOL_VERSION;
    return capabilities;
}
