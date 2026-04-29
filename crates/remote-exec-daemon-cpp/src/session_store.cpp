#include <atomic>
#include <cctype>
#include <cstdint>
#include <sstream>
#include <string>

#include "logging.h"
#include "platform.h"
#include "process_session.h"
#include "session_store.h"

namespace {

std::atomic<unsigned long> next_id(1UL);

std::string make_chunk_id() {
    std::ostringstream out;
    out << "cpp-" << platform::monotonic_ms() << '-' << next_id.fetch_add(1UL);
    return out.str();
}

struct PollResult {
    std::string output;
    bool completed;
    int exit_code;
};

bool is_token_space(char ch) {
    return std::isspace(static_cast<unsigned char>(ch)) != 0;
}

unsigned long count_tokens(const std::string& output) {
    unsigned long count = 0;
    bool in_token = false;
    for (std::size_t i = 0; i < output.size(); ++i) {
        if (is_token_space(output[i])) {
            in_token = false;
            continue;
        }
        if (!in_token) {
            ++count;
            in_token = true;
        }
    }
    return count;
}

std::string trim_trailing_token_space(std::string output) {
    while (!output.empty() && is_token_space(output[output.size() - 1])) {
        output.erase(output.size() - 1);
    }
    return output;
}

std::string truncate_output_tokens(const std::string& output, unsigned long max_output_tokens) {
    if (max_output_tokens == 0) {
        return "";
    }

    unsigned long seen = 0;
    bool in_token = false;
    for (std::size_t i = 0; i < output.size(); ++i) {
        if (is_token_space(output[i])) {
            in_token = false;
            continue;
        }
        if (!in_token) {
            ++seen;
            if (seen > max_output_tokens) {
                return trim_trailing_token_space(output.substr(0, i));
            }
            in_token = true;
        }
    }

    return output;
}

double wall_time_seconds(std::uint64_t started_at_ms) {
    const std::uint64_t now = platform::monotonic_ms();
    if (now < started_at_ms) {
        return 0.0;
    }
    return static_cast<double>(now - started_at_ms) / 1000.0;
}

Json build_response(
    const char* daemon_session_id,
    bool running,
    std::uint64_t started_at_ms,
    bool has_exit_code,
    int exit_code,
    const std::string& output,
    unsigned long max_output_tokens
) {
    const std::string trimmed = truncate_output_tokens(output, max_output_tokens);
    const unsigned long original_token_count = count_tokens(output);
    return Json{
        {"daemon_session_id", daemon_session_id != NULL ? Json(daemon_session_id) : Json(nullptr)},
        {"running", running},
        {"chunk_id", make_chunk_id()},
        {"wall_time_seconds", wall_time_seconds(started_at_ms)},
        {"exit_code", has_exit_code ? Json(exit_code) : Json(nullptr)},
        {"original_token_count", original_token_count},
        {"output", trimmed},
        {"warnings", Json::array()}
    };
}

std::shared_ptr<LiveSession> launch_live_session(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login,
    bool tty
) {
    std::shared_ptr<LiveSession> session(new LiveSession());
    session->id = make_chunk_id();
    session->process = ProcessSession::launch(command, workdir, shell, login, tty);
    session->started_at_ms = platform::monotonic_ms();
#ifdef _WIN32
    session->stdin_open = true;
#else
    session->stdin_open = tty;
#endif
    return session;
}

std::string read_available(const std::shared_ptr<LiveSession>& session) {
    return session->process->read_available(&session->output_carry);
}

std::string flush_output_carry(const std::shared_ptr<LiveSession>& session) {
    return session->process->flush_carry(&session->output_carry);
}

PollResult poll_session(
    const std::shared_ptr<LiveSession>& session,
    unsigned long timeout_ms
) {
    const std::uint64_t poll_start = platform::monotonic_ms();
    std::string output;

    for (;;) {
        output += read_available(session);

        int exit_code = 0;
        if (session->process->has_exited(&exit_code)) {
            output += read_available(session);
            output += flush_output_carry(session);
            return PollResult{output, true, exit_code};
        }

        if (platform::monotonic_ms() - poll_start >= timeout_ms) {
            return PollResult{output, false, 0};
        }

        platform::sleep_ms(25);
    }
}

}  // namespace

LiveSession::LiveSession() : started_at_ms(0), stdin_open(false) {}

LiveSession::~LiveSession() {}

SessionStore::SessionStore() {}

SessionStore::~SessionStore() {
    for (std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.begin();
         it != sessions_.end();
         ++it) {
        if (it->second->process.get() != NULL) {
            it->second->process->terminate();
        }
    }
}

Json SessionStore::start_command(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login,
    bool tty,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time,
    unsigned long max_open_sessions
) {
    if (sessions_.size() >= max_open_sessions) {
        throw SessionLimitError("too many open exec sessions");
    }

    {
        std::ostringstream message;
        message << "start_command cmd_preview=`" << preview_text(command, 120)
                << "` workdir=`" << workdir << "` shell=`" << shell
                << "` login=" << (login ? "true" : "false")
                << " tty=" << (tty ? "true" : "false");
        log_message(LOG_INFO, "session_store", message.str());
    }
    std::shared_ptr<LiveSession> session =
        launch_live_session(command, workdir, shell, login, tty);

    const unsigned long timeout_ms = resolve_yield_time_ms(
        yield_time.exec_command,
        has_yield_time_ms,
        yield_time_ms
    );
    const PollResult poll_result = poll_session(session, timeout_ms);

    if (poll_result.completed) {
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_tokens
        );
        {
            std::ostringstream message;
            message << "command completed before session handoff exit_code="
                    << poll_result.exit_code
                    << " output_chars=" << poll_result.output.size();
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
    }

    sessions_[session->id] = session;
    {
        std::ostringstream message;
        message << "stored live session daemon_session_id=`" << session->id
                << "` open_sessions=" << sessions_.size();
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_response(
        session->id.c_str(),
        true,
        session->started_at_ms,
        false,
        0,
        poll_result.output,
        max_output_tokens
    );
}

Json SessionStore::write_stdin(
    const std::string& daemon_session_id,
    const std::string& chars,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time
) {
    std::map<std::string, std::shared_ptr<LiveSession> >::iterator it =
        sessions_.find(daemon_session_id);
    if (it == sessions_.end()) {
        log_message(
            LOG_WARN,
            "session_store",
            "unknown daemon session `" + daemon_session_id + "`"
        );
        throw UnknownSessionError("Unknown daemon session");
    }

    {
        std::ostringstream message;
        message << "write_stdin daemon_session_id=`" << daemon_session_id
                << "` chars_len=" << chars.size();
        log_message(LOG_INFO, "session_store", message.str());
    }

    const std::shared_ptr<LiveSession>& session = it->second;
    if (!chars.empty()) {
        if (!session->stdin_open) {
            throw StdinClosedError(
                "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
            );
        }
        session->process->write_stdin(chars);
    }

    const YieldTimeOperationConfig& operation_config =
        chars.empty() ? yield_time.write_stdin_poll : yield_time.write_stdin_input;
    const unsigned long timeout_ms = resolve_yield_time_ms(
        operation_config,
        has_yield_time_ms,
        yield_time_ms
    );
    const PollResult poll_result = poll_session(session, timeout_ms);

    if (poll_result.completed) {
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_tokens
        );
        sessions_.erase(it);
        {
            std::ostringstream message;
            message << "session completed daemon_session_id=`" << daemon_session_id
                    << "` exit_code=" << poll_result.exit_code
                    << " open_sessions=" << sessions_.size();
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
    }

    {
        std::ostringstream message;
        message << "session still running daemon_session_id=`" << session->id << '`';
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_response(
        session->id.c_str(),
        true,
        session->started_at_ms,
        false,
        0,
        poll_result.output,
        max_output_tokens
    );
}
