#include <string>

#include "core/logging.h"
#include "rpc/exec_request_utils.h"
#include "rpc/server_route_common.h"
#include "rpc/server_route_exec.h"

HttpResponse handle_exec_start(AppState& state, const HttpRequest& request) {
    return handle_exec_rpc_route("exec/start", ExecRouteKind::Start, [&](HttpResponse& response) {
        const ExecStartRequestSpec parsed = prepare_exec_start_request(state, request);
        Json exec_response = state.services.sessions.start_command(
            state.config.target, parsed, state.config.yield_time, state.config.max_open_sessions);
        log_message(LOG_INFO,
                    "server",
                    "exec/start target=`" + state.config.target + "` cmd_preview=`" + preview_text(parsed.cmd, 120) +
                        "`");
        exec_response["daemon_instance_id"] = state.metadata.daemon_instance_id;
        write_json(response, exec_response);
    });
}

HttpResponse handle_exec_write(AppState& state, const HttpRequest& request) {
    return handle_exec_rpc_route("exec/write", ExecRouteKind::Write, [&](HttpResponse& response) {
        const ExecWriteRequestSpec parsed = prepare_exec_write_request(request);
        {
            LogMessageBuilder message("exec/write");
            message.quoted_field("daemon_session_id", parsed.daemon_session_id).field("chars_len", parsed.chars.size());
            log_message(LOG_INFO, "server", message.str());
        }
        Json exec_response = state.services.sessions.write_stdin(parsed.daemon_session_id,
                                                                 parsed.chars,
                                                                 parsed.has_yield_time_ms,
                                                                 parsed.yield_time_ms,
                                                                 parsed.max_output_tokens,
                                                                 state.config.yield_time,
                                                                 parsed.pty_size.present,
                                                                 parsed.pty_size.rows,
                                                                 parsed.pty_size.cols);
        exec_response["daemon_instance_id"] = state.metadata.daemon_instance_id;
        write_json(response, exec_response);
    });
}
