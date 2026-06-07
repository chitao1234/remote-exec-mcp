#pragma once

#include "http/http_helpers.h"
#include "http/server_transport.h"
#include "rpc/transfer_request_utils.h"
#include "runtime/route_context.h"

struct StreamingTransferExport {
    TransferExportRequestSpec request;
    ExportedPayload response_payload;
};

HttpResponse handle_transfer_export(const TransferRouteContext& context, const HttpRequest& request);
HttpResponse handle_transfer_path_info(const TransferRouteContext& context, const HttpRequest& request);
HttpResponse handle_transfer_import(const TransferRouteContext& context, const HttpRequest& request);
HttpResponse handle_streaming_transfer_import(
    const TransferRouteContext& context,
    const HttpRequest& request,
    HttpRequestBodyStream* body
);
HttpResponse prepare_streaming_transfer_export(
    const TransferRouteContext& context,
    const HttpRequest& request_head,
    HttpRequestBodyStream* body,
    StreamingTransferExport* transfer
);
void run_streaming_transfer_export(const StreamingTransferExport& transfer, HttpChunkedResponseWriter* chunks);
