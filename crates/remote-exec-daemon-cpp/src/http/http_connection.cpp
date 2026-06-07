#include <algorithm>
#include <cstdint>
#include <limits>
#include <string>

#include "core/logging.h"
#include "core/text_utils.h"
#include "http/http_connection.h"
#include "http/http_helpers.h"
#include "http/http_request.h"
#include "http/server_transport.h"
#include "platform/platform.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_executor.h"

namespace {

const unsigned long HTTP_SHUTDOWN_READ_POLL_MS = 100UL;
const unsigned long HTTP_STREAMING_TRANSFER_DRAIN_TIMEOUT_MS = 300000UL;

bool request_connection_close_requested(const HttpRequest& request) {
    const std::string value = lowercase_ascii(request.header("connection"));
    std::size_t offset = 0;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token =
            trim_ascii(comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset));
        if (token == "close") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }

    return false;
}

bool log_send_failure(const SocketSendError& ex) {
    if (ex.peer_disconnected()) {
        log_message(LOG_WARN, "server", std::string("client disconnected during send: ") + ex.what());
        return true;
    }

    log_message(LOG_ERROR, "server", std::string("send failed: ") + ex.what());
    return false;
}

bool try_send_response(SOCKET client, const HttpResponse& response) {
    try {
        send_http_response(client, response);
        return true;
    } catch (const SocketSendError& ex) {
        log_send_failure(ex);
        return false;
    }
}

bool try_send_response_head(SOCKET client, const HttpResponse& response) {
    try {
        send_http_response_head(client, response);
        return true;
    } catch (const SocketSendError& ex) {
        log_send_failure(ex);
        return false;
    }
}

HttpRequestBodyFraming parse_request_body_framing_or_throw_bad_request(const HttpRequest& request) {
    try {
        return request_body_framing_from_headers(request.headers);
    } catch (const HttpProtocolError& ex) {
        throw BadHttpRequest(ex.what());
    }
}

void log_request_result(const HttpRequest& request, int status, std::uint64_t started_at_ms) {
    LogMessageBuilder message(request.method + " " + request.path);
    message.field("request_id", request_id_for_request(request))
        .field("status", status)
        .field("elapsed_ms", (platform::monotonic_ms() - started_at_ms));
    const LogLevel level = request.path == "/v1/health" ? LOG_DEBUG : level_for_status(status);
    log_message(level, "server", message.str());
}

bool try_send_upgrade_response(
    SOCKET client,
    const std::string& upgrade_token,
    const std::map<std::string, std::string>& headers
) {
    try {
        send_http_upgrade_response(client, upgrade_token, headers);
        return true;
    } catch (const SocketSendError& ex) {
        log_send_failure(ex);
        return false;
    }
}

int handle_client_request(
    const HttpConnectionContext& context,
    SOCKET client,
    const HttpRequestHead& request_head,
    const HttpReadControl& read_control,
    bool* close_after_response
) {
    const std::uint64_t started_at_ms = platform::monotonic_ms();
    HttpRequest request = parse_http_request_head(request_head.raw_headers);
    assign_request_id(request);
    *close_after_response = request_connection_close_requested(request);
    const HttpRequestBodyFraming framing = parse_request_body_framing_or_throw_bad_request(request);
    const RpcRouteBodyPolicy body_policy = rpc_route_body_policy(request, context.max_request_body_bytes);
    HttpReadControl body_read_control = read_control;
    if (body_policy.min_idle_timeout_ms > body_read_control.idle_timeout_ms) {
        body_read_control.idle_timeout_ms =
            std::max(body_read_control.idle_timeout_ms, body_policy.min_idle_timeout_ms);
        set_socket_timeout_ms(client, body_read_control.idle_timeout_ms);
    }
    HttpRequestBodyStream
        body(client, request_head.initial_body, framing, body_policy.max_body_bytes, &body_read_control);

    const RpcRouteExecution execution = execute_rpc_route(context.routes, context.port_tunnel, request, &body);
    if (execution.kind == RPC_ROUTE_EXECUTION_STREAMING_IMPORT_RESPONSE && execution.close_after_response) {
        (void)body.discard_remaining_bounded(
            HTTP_STREAMING_TRANSFER_DRAIN_TIMEOUT_MS,
            std::numeric_limits<std::size_t>::max()
        );
        *close_after_response = true;
    }

    if (execution.kind == RPC_ROUTE_EXECUTION_STREAMING_RESPONSE) {
        log_request_result(request, execution.response.status, started_at_ms);
        if (!try_send_response_head(client, execution.response)) {
            *close_after_response = true;
            return execution.response.status;
        }
        HttpChunkedResponseWriter chunks(client);
        execution.write_streaming_response(&chunks);
        return execution.response.status;
    }

    if (execution.kind == RPC_ROUTE_EXECUTION_UPGRADE_HANDLER) {
        log_request_result(request, execution.response.status, started_at_ms);
        *close_after_response = true;
        if (!try_send_upgrade_response(client, execution.upgrade_token, execution.upgrade_headers)) {
            return execution.response.status;
        }
        execution.run_upgrade(client);
        return execution.response.status;
    }

    if (execution.close_after_response) {
        *close_after_response = true;
    }
    log_request_result(request, execution.response.status, started_at_ms);
    if (!try_send_response(client, execution.response)) {
        *close_after_response = true;
    }
    return execution.response.status;
}

} // namespace

void handle_client(const HttpConnectionContext& context, UniqueSocket client) {
    HttpReadControl read_control;
    read_control.idle_timeout_ms = context.http_connection_idle_timeout_ms;
    read_control.poll_timeout_ms = HTTP_SHUTDOWN_READ_POLL_MS;
    read_control.stop_requested = [&context]() { return context.shutdown_requested.load(); };

    for (;;) {
        try {
            set_socket_timeout_ms(client.get(), context.http_connection_idle_timeout_ms);
            HttpRequestHead request_head;
            if (!try_read_http_request_head_controlled(
                    client.get(),
                    context.max_request_header_bytes,
                    read_control,
                    &request_head
                )) {
                return;
            }
            set_socket_timeout_ms(client.get(), context.http_connection_idle_timeout_ms);

            bool close_after_response = false;
            handle_client_request(context, client.get(), request_head, read_control, &close_after_response);
            if (close_after_response) {
                return;
            }
        } catch (const HttpConnectionShutdown&) {
            return;
        } catch (const BadHttpRequest& ex) {
            log_message(LOG_WARN, "server", ex.what());
            HttpResponse response;
            response.status = 400;
            write_rpc_error(response, 400, "bad_request", ex.what());
            try_send_response(client.get(), response);
            return;
        } catch (const HttpParseError& ex) {
            log_message(LOG_WARN, "server", ex.what());
            HttpResponse response;
            response.status = 400;
            write_rpc_error(response, 400, "bad_request", ex.what());
            try_send_response(client.get(), response);
            return;
        } catch (const SocketSendError& ex) {
            log_send_failure(ex);
            return;
        } catch (const std::exception& ex) {
            log_message(LOG_ERROR, "server", ex.what());
            HttpResponse response;
            response.status = 500;
            write_rpc_error(response, 500, "internal_error", ex.what());
            try_send_response(client.get(), response);
            return;
        }
    }
}
