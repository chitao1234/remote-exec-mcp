#pragma once

#include "http/http_helpers.h"
#include "rpc/transfer_request_utils.h"
#include "runtime/route_context.h"

HttpResponse handle_transfer_export(
    const TransferRouteContext& context,
    const HttpRequest& request
);
HttpResponse handle_transfer_path_info(
    const TransferRouteContext& context,
    const HttpRequest& request
);
HttpResponse handle_transfer_import(
    const TransferRouteContext& context,
    const HttpRequest& request
);
