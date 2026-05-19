#include "capabilities/daemon_capabilities.h"

#include "exec/process_session.h"
#include "rpc/server_contract.h"
#include "runtime/server.h"

namespace {

bool port_forward_runtime_available(const AppState& state) {
    return state.port_tunnel_service.get() != nullptr;
}

} // namespace

DaemonCapabilities detect_daemon_capabilities(const AppState& state) {
    DaemonCapabilities capabilities;
    capabilities.supports_pty = process_session_supports_pty();
    capabilities.supports_image_read = true;
    capabilities.supports_transfer_compression = false;
    capabilities.supports_port_forward = port_forward_runtime_available(state);
    capabilities.port_forward_protocol_version =
        capabilities.supports_port_forward ? server_contract::PORT_TUNNEL_PROTOCOL_VERSION : 0U;
    return capabilities;
}

void write_daemon_capabilities(Json* target, const DaemonCapabilities& capabilities) {
    (*target)["supports_pty"] = capabilities.supports_pty;
    (*target)["supports_image_read"] = capabilities.supports_image_read;
    (*target)["supports_transfer_compression"] = capabilities.supports_transfer_compression;
    (*target)["supports_port_forward"] = capabilities.supports_port_forward;
    (*target)["port_forward_protocol_version"] = capabilities.port_forward_protocol_version;
}
