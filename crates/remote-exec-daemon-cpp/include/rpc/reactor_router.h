#pragma once

#include "core/config.h"
#include "http/http_helpers.h"
#include "rpc/server_contract.h"
#include "rpc/server_route_transfer.h"
#include "runtime/app_state.h"
#include "runtime/route_context.h"

// Buffered-route dispatcher for the reactor server. It borrows config,
// metadata, and sandbox state from the server that owns it.
class ReactorRouter {
public:
    ReactorRouter(
        const DaemonConfig& config,
        const AppMetadata& metadata,
        const AppSandboxState& sandbox
    );

    HttpResponse dispatch(server_contract::RouteId route, const HttpRequest& request) const;

    // Routes that never block and can run on the loop thread.
    static bool runs_inline(server_contract::RouteId route);

    // Routes implemented on the reactor server so far.
    static bool is_migrated(server_contract::RouteId route);

private:
    HttpGateContext gate_;
    PathResolutionContext paths_;
    HealthRouteContext health_;
    TargetInfoRouteContext target_info_;
    FileRouteContext file_;
    ImageRouteContext image_;
    PatchRouteContext patch_;
    TransferRouteContext transfer_;
};
