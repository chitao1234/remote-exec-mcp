#include "rpc/capabilities_http_codec.h"

void write_daemon_capabilities(Json* target, const DaemonCapabilities& capabilities) {
    (*target)["supports_pty"] = capabilities.supports_pty;
    (*target)["supports_image_read"] = capabilities.supports_image_read;
    (*target)["supports_transfer_compression"] = capabilities.supports_transfer_compression;
    (*target)["supports_port_forward"] = capabilities.supports_port_forward;
    (*target)["port_forward_protocol_version"] = capabilities.port_forward_protocol_version;
    (*target)["transfer_stream_protocol_version"] = capabilities.transfer_stream_protocol_version;
}
