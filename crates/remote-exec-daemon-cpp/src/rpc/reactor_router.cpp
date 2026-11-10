#include "rpc/reactor_router.h"

#include <string>

#include "patch/patch_engine.h"
#include "policy/file_ops.h"
#include "rpc/server_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_file.h"
#include "rpc/server_route_image.h"

namespace {

TextDecodeSettings make_text_decode(const DaemonConfig& config) {
    TextDecodeSettings settings;
    settings.allow_encoding_autodetect = config.experimental_apply_patch_target_encoding_autodetect;
    return settings;
}

PatchApplyOptions make_patch_options(const DaemonConfig& config) {
    PatchApplyOptions options;
    options.allow_encoding_autodetect = config.experimental_apply_patch_target_encoding_autodetect;
    return options;
}

} // namespace

ReactorRouter::ReactorRouter(
    const DaemonConfig& config,
    const AppMetadata& metadata,
    const AppSandboxState& sandbox
)
    : gate_(config.http_auth_bearer_token),
      paths_(config.default_workdir, config.windows_posix_root, sandbox.active()),
      health_(metadata.daemon_instance_id), target_info_(
                                                config.target,
                                                metadata.daemon_instance_id,
                                                metadata.hostname,
                                                metadata.capabilities
                                            ),
      file_(paths_, make_text_decode(config)), image_(paths_), patch_(
                                                                   paths_,
                                                                   metadata.daemon_instance_id,
                                                                   config.allow_apply_patch,
                                                                   make_patch_options(config)
                                                               ),
      transfer_(gate_, paths_, config.transfer_limits) {
}

bool ReactorRouter::runs_inline(server_contract::RouteId route) {
    return route == server_contract::ROUTE_HEALTH || route == server_contract::ROUTE_TARGET_INFO;
}

bool ReactorRouter::is_migrated(server_contract::RouteId route) {
    switch (route) {
    case server_contract::ROUTE_HEALTH:
    case server_contract::ROUTE_TARGET_INFO:
    case server_contract::ROUTE_IMAGE_READ:
    case server_contract::ROUTE_PATCH_APPLY:
    case server_contract::ROUTE_FILE_READ:
    case server_contract::ROUTE_FILE_WRITE:
    case server_contract::ROUTE_FILE_EDIT:
    case server_contract::ROUTE_TRANSFER_EXPORT:
    case server_contract::ROUTE_TRANSFER_PATH_INFO:
    case server_contract::ROUTE_TRANSFER_IMPORT:
        return true;
    default:
        return false;
    }
}

HttpResponse ReactorRouter::dispatch(server_contract::RouteId route, const HttpRequest& request)
    const {
    HttpResponse response;
    if (reject_before_route(gate_, request, &response)) {
        write_request_id_header(response, request);
        return response;
    }

    switch (route) {
    case server_contract::ROUTE_HEALTH:
        response = handle_health(health_);
        break;
    case server_contract::ROUTE_TARGET_INFO:
        response = handle_target_info(target_info_);
        break;
    case server_contract::ROUTE_IMAGE_READ:
        response = handle_image_read(image_, request);
        break;
    case server_contract::ROUTE_PATCH_APPLY:
        response = handle_patch_apply(patch_, request);
        break;
    case server_contract::ROUTE_FILE_READ:
        response = handle_file_read(file_, request);
        break;
    case server_contract::ROUTE_FILE_WRITE:
        response = handle_file_write(file_, request);
        break;
    case server_contract::ROUTE_FILE_EDIT:
        response = handle_file_edit(file_, request);
        break;
    case server_contract::ROUTE_TRANSFER_EXPORT:
        response = handle_transfer_export(transfer_, request);
        break;
    case server_contract::ROUTE_TRANSFER_PATH_INFO:
        response = handle_transfer_path_info(transfer_, request);
        break;
    case server_contract::ROUTE_TRANSFER_IMPORT:
        response = handle_transfer_import(transfer_, request);
        break;
    default:
        response = make_rpc_error_response(
            501,
            "not_migrated",
            "route is not available on the reactor server yet"
        );
        break;
    }

    write_request_id_header(response, request);
    return response;
}
