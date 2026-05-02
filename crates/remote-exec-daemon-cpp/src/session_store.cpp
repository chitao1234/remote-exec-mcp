#include <atomic>
#include <cctype>
#include <cstdint>
#include <sstream>
#include <string>
#include <vector>

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

const unsigned long EXIT_DRAIN_INITIAL_WAIT_MS = 125UL;
const unsigned long EXIT_DRAIN_QUIET_MS = 25UL;

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

std::string drain_output_after_exit(
    const std::shared_ptr<LiveSession>& session,
    std::uint64_t poll_start,
    unsigned long timeout_ms,
    bool saw_output_before_exit
) {
    const std::uint64_t drain_start = platform::monotonic_ms();
    std::uint64_t last_output_at = drain_start;
    bool saw_output = saw_output_before_exit;
    std::string output;

    for (;;) {
        const std::string chunk = read_available(session);
        if (!chunk.empty()) {
            output += chunk;
            saw_output = true;
            last_output_at = platform::monotonic_ms();
            continue;
        }

        const std::uint64_t now = platform::monotonic_ms();
        if (now - poll_start >= timeout_ms) {
            return output;
        }
        if (saw_output) {
            if (now - last_output_at >= EXIT_DRAIN_QUIET_MS) {
                return output;
            }
        } else if (now - drain_start >= EXIT_DRAIN_INITIAL_WAIT_MS) {
            return output;
        }

        platform::sleep_ms(10);
    }
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
            const bool saw_output_before_exit = !output.empty();
            output += read_available(session);
            output += drain_output_after_exit(
                session,
                poll_start,
                timeout_ms,
                saw_output_before_exit || !output.empty()
            );
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

LiveSession::LiveSession() : started_at_ms(0), stdin_open(false), retired(false) {}

LiveSession::~LiveSession() {}

SessionStore::SessionStore() : pending_starts_(0UL) {}

SessionStore::~SessionStore() {
    std::vector<std::shared_ptr<LiveSession> > sessions;
    {
        BasicLockGuard lock(mutex_);
        for (std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.begin();
             it != sessions_.end();
             ++it) {
            sessions.push_back(it->second);
        }
        sessions_.clear();
        pending_starts_ = 0UL;
    }

    for (std::size_t i = 0; i < sessions.size(); ++i) {
        BasicLockGuard session_lock(sessions[i]->mutex_);
        sessions[i]->retired = true;
        if (sessions[i]->process.get() != NULL) {
            sessions[i]->process->terminate();
        }
    }
}

void release_pending_start(BasicMutex& mutex, unsigned long* pending_starts) {
    BasicLockGuard lock(mutex);
    if (*pending_starts > 0UL) {
        --(*pending_starts);
    }
}

void erase_session_if_current(
    BasicMutex& mutex,
    std::map<std::string, std::shared_ptr<LiveSession> >& sessions,
    const std::string& daemon_session_id,
    const std::shared_ptr<LiveSession>& session
) {
    BasicLockGuard lock(mutex);
    std::map<std::string, std::shared_ptr<LiveSession> >::iterator it =
        sessions.find(daemon_session_id);
    if (it != sessions.end() && it->second == session) {
        sessions.erase(it);
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
    {
        BasicLockGuard lock(mutex_);
        if (sessions_.size() + pending_starts_ >= max_open_sessions) {
            throw SessionLimitError("too many open exec sessions");
        }
        ++pending_starts_;
    }

    {
        std::ostringstream message;
        message << "start_command cmd_preview=`" << preview_text(command, 120)
                << "` workdir=`" << workdir << "` shell=`" << shell
                << "` login=" << (login ? "true" : "false")
                << " tty=" << (tty ? "true" : "false");
        log_message(LOG_INFO, "session_store", message.str());
    }
    try {
        std::shared_ptr<LiveSession> session =
            launch_live_session(command, workdir, shell, login, tty);

        const unsigned long timeout_ms = resolve_yield_time_ms(
            yield_time.exec_command,
            has_yield_time_ms,
            yield_time_ms
        );
        PollResult poll_result;
        {
            BasicLockGuard session_lock(session->mutex_);
            poll_result = poll_session(session, timeout_ms);
            if (poll_result.completed) {
                session->retired = true;
            }
        }

        {
            BasicLockGuard lock(mutex_);
            if (pending_starts_ > 0UL) {
                --pending_starts_;
            }
            if (!poll_result.completed) {
                sessions_[session->id] = session;
                std::ostringstream message;
                message << "stored live session daemon_session_id=`" << session->id
                        << "` open_sessions=" << sessions_.size();
                log_message(LOG_INFO, "session_store", message.str());
            }
        }

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

        return build_response(
            session->id.c_str(),
            true,
            session->started_at_ms,
            false,
            0,
            poll_result.output,
            max_output_tokens
        );
    } catch (...) {
        release_pending_start(mutex_, &pending_starts_);
        throw;
    }
}

Json SessionStore::write_stdin(
    const std::string& daemon_session_id,
    const std::string& chars,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time
) {
    std::shared_ptr<LiveSession> session;
    {
        BasicLockGuard lock(mutex_);
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
        session = it->second;
    }

    {
        std::ostringstream message;
        message << "write_stdin daemon_session_id=`" << daemon_session_id
                << "` chars_len=" << chars.size();
        log_message(LOG_INFO, "session_store", message.str());
    }

    PollResult poll_result;
    {
        BasicLockGuard session_lock(session->mutex_);
        if (session->retired) {
            log_message(
                LOG_WARN,
                "session_store",
                "unknown daemon session `" + daemon_session_id + "`"
            );
            throw UnknownSessionError("Unknown daemon session");
        }

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
        poll_result = poll_session(session, timeout_ms);
        if (poll_result.completed) {
            session->retired = true;
        }
    }

    if (poll_result.completed) {
        erase_session_if_current(mutex_, sessions_, daemon_session_id, session);
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
            unsigned long open_sessions = 0UL;
            {
                BasicLockGuard lock(mutex_);
                open_sessions =
                    static_cast<unsigned long>(sessions_.size() + pending_starts_);
            }
            message << "session completed daemon_session_id=`" << daemon_session_id
                    << "` exit_code=" << poll_result.exit_code
                    << " open_sessions=" << open_sessions;
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
