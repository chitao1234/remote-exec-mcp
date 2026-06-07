#include <string>

#include "core/logging.h"
#include "rpc/exec_http_codec.h"
#include "rpc/exec_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_exec.h"

HttpResponse handle_exec_start(const ExecRouteContext& context, const HttpRequest& request) {
    return handle_exec_rpc_route("exec/start", ExecRouteKind::Start, [&](HttpResponse& response) {
        const ExecStartRequestSpec parsed = prepare_exec_start_request(context.request, request);
        const ExecSessionResult exec_result =
            context.sessions.start_command(context.target, parsed, context.yield_time, context.max_open_sessions);
        log_message(
            LOG_INFO,
            "server",
            "exec/start target=`" + context.target + "` cmd_preview=`" + preview_text(parsed.cmd, 120) + "`"
        );
        Json exec_response = exec_session_result_json(exec_result, parsed.max_output_tokens);
        exec_response["daemon_instance_id"] = context.daemon_instance_id;
        write_json(response, exec_response);
    });
}

HttpResponse handle_exec_write(const ExecRouteContext& context, const HttpRequest& request) {
    return handle_exec_rpc_route("exec/write", ExecRouteKind::Write, [&](HttpResponse& response) {
        const ExecWriteRequestSpec parsed = prepare_exec_write_request(request);
        {
            LogMessageBuilder message("exec/write");
            message.quoted_field("daemon_session_id", parsed.daemon_session_id).field("chars_len", parsed.chars.size());
            log_message(LOG_INFO, "server", message.str());
        }
        const ExecSessionResult exec_result = context.sessions.write_stdin(
            parsed.daemon_session_id,
            parsed.chars,
            parsed.has_yield_time_ms,
            parsed.yield_time_ms,
            parsed.max_output_tokens,
            context.yield_time,
            parsed.pty_size.present,
            parsed.pty_size.rows,
            parsed.pty_size.cols
        );
        Json exec_response = exec_session_result_json(exec_result, parsed.max_output_tokens);
        exec_response["daemon_instance_id"] = context.daemon_instance_id;
        write_json(response, exec_response);
    });
}
