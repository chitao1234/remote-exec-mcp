#include "capabilities/daemon_capabilities.h"

#include "exec/process_session.h"
#include "rpc/server_contract.h"

DaemonCapabilities::DaemonCapabilities()
    : supports_pty(false), supports_image_read(false), supports_transfer_compression(false),
      supports_port_forward(false), port_forward_protocol_version(0U), transfer_stream_protocol_version(0U) {
}

DaemonCapabilities detect_daemon_capabilities() {
    DaemonCapabilities capabilities;
    capabilities.supports_pty = process_session_supports_pty();
    capabilities.supports_image_read = true;
    capabilities.supports_transfer_compression = false;
    capabilities.supports_port_forward = true;
    capabilities.port_forward_protocol_version = server_contract::PORT_TUNNEL_PROTOCOL_VERSION;
    capabilities.transfer_stream_protocol_version = server_contract::TRANSFER_STREAM_PROTOCOL_VERSION;
    return capabilities;
}

void write_daemon_capabilities(Json* target, const DaemonCapabilities& capabilities) {
    (*target)["supports_pty"] = capabilities.supports_pty;
    (*target)["supports_image_read"] = capabilities.supports_image_read;
    (*target)["supports_transfer_compression"] = capabilities.supports_transfer_compression;
    (*target)["supports_port_forward"] = capabilities.supports_port_forward;
    (*target)["port_forward_protocol_version"] = capabilities.port_forward_protocol_version;
    (*target)["transfer_stream_protocol_version"] = capabilities.transfer_stream_protocol_version;
}
