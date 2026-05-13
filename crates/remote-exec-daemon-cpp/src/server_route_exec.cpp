#include <string>

#include "logging.h"
#include "process_session.h"
#include "server_request_utils.h"
#include "server_route_common.h"
#include "server_route_exec.h"

HttpResponse handle_exec_start(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const ExecStartRequestSpec parsed = prepare_exec_start_request(state, request);
        Json exec_response = state.sessions.start_command(state.config.target,
                                                          parsed.cmd,
                                                          parsed.workdir,
                                                          parsed.shell,
                                                          parsed.login_requested,
                                                          parsed.tty_requested,
                                                          parsed.has_yield_time_ms,
                                                          parsed.yield_time_ms,
                                                          parsed.max_output_tokens,
                                                          state.config.yield_time,
                                                          state.config.max_open_sessions);
        log_message(LOG_INFO,
                    "server",
                    "exec/start target=`" + state.config.target + "` cmd_preview=`" +
                        preview_text(parsed.cmd, 120) + "`");
        exec_response["daemon_instance_id"] = state.daemon_instance_id;
        write_json(response, exec_response);
    } catch (const SessionLimitError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/start rejected: ") + ex.what());
        write_rpc_error(response, 429, "session_limit_exceeded", ex.what());
    } catch (const SandboxError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/start denied: ") + ex.what());
        write_rpc_error(response, 400, "sandbox_denied", ex.what());
    } catch (const ExecRequestFailure& ex) {
        log_message(level_for_status(ex.status), "server", std::string("exec/start rejected: ") + ex.what());
        write_rpc_error(response, ex.status, ex.code, ex.message);
    } catch (const std::exception& ex) {
        log_message(LOG_ERROR, "server", std::string("exec/start failed: ") + ex.what());
        write_rpc_error(response, 500, "internal_error", ex.what());
    }

    return response;
}

HttpResponse handle_exec_write(AppState& state, const HttpRequest& request) {
    HttpResponse response;
    response.status = 200;

    try {
        const ExecWriteRequestSpec parsed = prepare_exec_write_request(request);
        {
            LogMessageBuilder message("exec/write");
            message.quoted_field("daemon_session_id", parsed.daemon_session_id)
                .field("chars_len", parsed.chars.size());
            log_message(LOG_INFO, "server", message.str());
        }
        Json exec_response = state.sessions.write_stdin(parsed.daemon_session_id,
                                                        parsed.chars,
                                                        parsed.has_yield_time_ms,
                                                        parsed.yield_time_ms,
                                                        parsed.max_output_tokens,
                                                        state.config.yield_time,
                                                        parsed.pty_size.present,
                                                        parsed.pty_size.rows,
                                                        parsed.pty_size.cols);
        exec_response["daemon_instance_id"] = state.daemon_instance_id;
        write_json(response, exec_response);
    } catch (const UnknownSessionError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write unknown session: ") + ex.what());
        write_rpc_error(response, 400, "unknown_session", ex.what());
    } catch (const StdinClosedError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write stdin closed: ") + ex.what());
        write_rpc_error(response, 400, "stdin_closed", ex.what());
    } catch (const ProcessPtyResizeUnsupportedError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write pty resize unsupported: ") + ex.what());
        write_rpc_error(response, 400, "tty_unsupported", ex.what());
    } catch (const ExecRequestFailure& ex) {
        log_message(level_for_status(ex.status), "server", std::string("exec/write rejected: ") + ex.what());
        write_rpc_error(response, ex.status, ex.code, ex.message);
    } catch (const std::exception& ex) {
        const std::string message = ex.what();
        log_message(LOG_ERROR, "server", std::string("exec/write failed: ") + message);
        write_rpc_error(response, 500, "internal_error", message);
    }

    return response;
}
