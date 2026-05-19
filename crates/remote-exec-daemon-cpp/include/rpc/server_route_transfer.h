#pragma once

#include "http/http_helpers.h"
#include "http/server_transport.h"
#include "runtime/server.h"

HttpResponse handle_transfer_export(AppState& state, const HttpRequest& request);
HttpResponse handle_transfer_path_info(AppState& state, const HttpRequest& request);
HttpResponse handle_transfer_import(AppState& state, const HttpRequest& request);
HttpResponse handle_streaming_transfer_import(const AppState& state,
                                              const HttpRequest& request,
                                              HttpRequestBodyStream* body);
int handle_streaming_transfer_export(const AppState& state,
                                     const HttpRequest& request_head,
                                     HttpRequestBodyStream* body,
                                     SOCKET client);
