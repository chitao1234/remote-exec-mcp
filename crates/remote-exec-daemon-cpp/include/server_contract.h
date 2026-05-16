#pragma once

#include <string>

namespace server_contract {

enum RouteId {
    ROUTE_UNKNOWN = 0,
    ROUTE_HEALTH,
    ROUTE_TARGET_INFO,
    ROUTE_IMAGE_READ,
    ROUTE_EXEC_START,
    ROUTE_EXEC_WRITE,
    ROUTE_PATCH_APPLY,
    ROUTE_TRANSFER_EXPORT,
    ROUTE_TRANSFER_PATH_INFO,
    ROUTE_TRANSFER_IMPORT,
    ROUTE_PORT_TUNNEL,
};

RouteId route_id_for_path(const std::string& path);
const char* route_path(RouteId id);

extern const char TRANSFER_DESTINATION_PATH_HEADER[];
extern const char TRANSFER_OVERWRITE_HEADER[];
extern const char TRANSFER_CREATE_PARENT_HEADER[];
extern const char TRANSFER_SOURCE_TYPE_HEADER[];
extern const char TRANSFER_COMPRESSION_HEADER[];
extern const char TRANSFER_SYMLINK_MODE_HEADER[];
extern const char TRANSFER_EXPORT_CONTENT_TYPE[];

extern const char PORT_TUNNEL_UPGRADE_TOKEN[];
extern const char PORT_TUNNEL_VERSION_HEADER[];
extern const char PORT_TUNNEL_VERSION_VALUE[];
extern const unsigned int PORT_TUNNEL_PROTOCOL_VERSION;

} // namespace server_contract
