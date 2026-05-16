#include <cstddef>

#include "server_contract.h"

namespace {

struct RouteEntry {
    server_contract::RouteId id;
    const char* path;
};

const RouteEntry ROUTE_ENTRIES[] = {
    {server_contract::ROUTE_HEALTH, "/v1/health"},
    {server_contract::ROUTE_TARGET_INFO, "/v1/target-info"},
    {server_contract::ROUTE_IMAGE_READ, "/v1/image/read"},
    {server_contract::ROUTE_EXEC_START, "/v1/exec/start"},
    {server_contract::ROUTE_EXEC_WRITE, "/v1/exec/write"},
    {server_contract::ROUTE_PATCH_APPLY, "/v1/patch/apply"},
    {server_contract::ROUTE_TRANSFER_EXPORT, "/v1/transfer/export"},
    {server_contract::ROUTE_TRANSFER_PATH_INFO, "/v1/transfer/path-info"},
    {server_contract::ROUTE_TRANSFER_IMPORT, "/v1/transfer/import"},
    {server_contract::ROUTE_PORT_TUNNEL, "/v1/port/tunnel"},
};

const std::size_t ROUTE_ENTRY_COUNT = sizeof(ROUTE_ENTRIES) / sizeof(ROUTE_ENTRIES[0]);

} // namespace

namespace server_contract {

const char TRANSFER_DESTINATION_PATH_HEADER[] = "x-remote-exec-destination-path";
const char TRANSFER_OVERWRITE_HEADER[] = "x-remote-exec-overwrite";
const char TRANSFER_CREATE_PARENT_HEADER[] = "x-remote-exec-create-parent";
const char TRANSFER_SOURCE_TYPE_HEADER[] = "x-remote-exec-source-type";
const char TRANSFER_COMPRESSION_HEADER[] = "x-remote-exec-compression";
const char TRANSFER_SYMLINK_MODE_HEADER[] = "x-remote-exec-symlink-mode";
const char TRANSFER_EXPORT_CONTENT_TYPE[] = "application/octet-stream";

const char PORT_TUNNEL_UPGRADE_TOKEN[] = "remote-exec-port-tunnel";
const char PORT_TUNNEL_VERSION_HEADER[] = "x-remote-exec-port-tunnel-version";
const char PORT_TUNNEL_VERSION_VALUE[] = "4";
const unsigned int PORT_TUNNEL_PROTOCOL_VERSION = 4U;

RouteId route_id_for_path(const std::string& path) {
    for (std::size_t i = 0; i < ROUTE_ENTRY_COUNT; ++i) {
        if (path == ROUTE_ENTRIES[i].path) {
            return ROUTE_ENTRIES[i].id;
        }
    }
    return ROUTE_UNKNOWN;
}

const char* route_path(RouteId id) {
    for (std::size_t i = 0; i < ROUTE_ENTRY_COUNT; ++i) {
        if (ROUTE_ENTRIES[i].id == id) {
            return ROUTE_ENTRIES[i].path;
        }
    }
    return "";
}

} // namespace server_contract
