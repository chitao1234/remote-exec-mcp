#include "rpc/exec_request_utils.h"

#include "http/http_helpers.h"
#include "platform/platform.h"
#include "rpc/server_request_utils.h"
#include "runtime/route_context.h"

namespace {

unsigned long requested_max_output_tokens(const Json& body) {
    const Json::const_iterator it = body.find("max_output_tokens");
    return it == body.end() ? DEFAULT_MAX_OUTPUT_TOKENS : it->get<unsigned long>();
}

ExecRequestFailure bad_request_failure(const Json::exception& ex) {
    return ExecRequestFailure(400, "bad_request", ex.what());
}

ExecPtySizeSpec requested_pty_size(const Json& body) {
    ExecPtySizeSpec result;
    result.present = false;
    result.rows = 0U;
    result.cols = 0U;

    const Json::const_iterator pty_size_it = body.find("pty_size");
    if (pty_size_it == body.end() || pty_size_it->is_null()) {
        return result;
    }

    try {
        const Json& pty_size = *pty_size_it;
        const unsigned long rows = pty_size.at("rows").get<unsigned long>();
        const unsigned long cols = pty_size.at("cols").get<unsigned long>();
        if (rows == 0UL || cols == 0UL || rows > 65535UL || cols > 65535UL) {
            throw ExecRequestFailure(
                400,
                "invalid_pty_size",
                "PTY rows and cols must be between 1 and 65535"
            );
        }
        result.present = true;
        result.rows = static_cast<unsigned short>(rows);
        result.cols = static_cast<unsigned short>(cols);
        return result;
    } catch (const ExecRequestFailure&) {
        throw;
    } catch (const Json::exception& ex) {
        throw ExecRequestFailure(
            400,
            "invalid_pty_size",
            std::string("invalid PTY size: ") + ex.what()
        );
    }
}

} // namespace

ExecRequestFailure::ExecRequestFailure(
    int status_value,
    const std::string& code_value,
    const std::string& message_value
)
    : std::runtime_error(message_value), status(status_value), code(code_value),
      message(message_value) {
}

ExecStartRequestSpec prepare_exec_start_request(
    const ExecRequestContext& context,
    const HttpRequest& request
) {
    try {
        const Json body = parse_json_body(request);
        ExecStartRequestSpec parsed;
        const Json::const_iterator yield_time_it = body.find("yield_time_ms");
        parsed.has_yield_time_ms = yield_time_it != body.end();
        parsed.yield_time_ms = parsed.has_yield_time_ms ? yield_time_it->get<unsigned long>() : 0UL;
        parsed.max_output_tokens = requested_max_output_tokens(body);
        parsed.cmd = body.at("cmd").get<std::string>();
        parsed.tty_requested = body.value("tty", false);
        if (parsed.tty_requested && !context.capabilities.supports_pty) {
            throw ExecRequestFailure(400, "tty_unsupported", "tty is not supported on this host");
        }

        parsed.login_requested = body.value("login", context.allow_login_shell);
        if (parsed.login_requested && !context.allow_login_shell) {
            throw ExecRequestFailure(
                400,
                "login_shell_disabled",
                "login shells are disabled by daemon config"
            );
        }

        const std::string shell_override = body.value("shell", std::string());
        if (!shell_override.empty() && !platform::shell_supported(shell_override)) {
            throw ExecRequestFailure(
                400,
                "unsupported_shell",
                "requested shell is not supported on this target"
            );
        }
        parsed.shell = platform::selected_shell(shell_override, context.default_shell);
        parsed.workdir = resolve_authorized_workdir(context.paths, body, SANDBOX_EXEC_CWD);
        return parsed;
    } catch (const ExecRequestFailure&) {
        throw;
    } catch (const Json::exception& ex) {
        throw bad_request_failure(ex);
    }
}

ExecWriteRequestSpec prepare_exec_write_request(const HttpRequest& request) {
    try {
        const Json body = parse_json_body(request);
        ExecWriteRequestSpec parsed;
        const Json::const_iterator yield_time_it = body.find("yield_time_ms");
        parsed.has_yield_time_ms = yield_time_it != body.end();
        parsed.yield_time_ms = parsed.has_yield_time_ms ? yield_time_it->get<unsigned long>() : 0UL;
        parsed.max_output_tokens = requested_max_output_tokens(body);
        parsed.daemon_session_id = body.at("daemon_session_id").get<std::string>();
        parsed.chars = body.value("chars", std::string());
        parsed.pty_size = requested_pty_size(body);
        return parsed;
    } catch (const ExecRequestFailure&) {
        throw;
    } catch (const Json::exception& ex) {
        throw bad_request_failure(ex);
    }
}
