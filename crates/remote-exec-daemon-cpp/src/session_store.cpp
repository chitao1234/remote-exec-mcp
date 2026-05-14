#include <atomic>
#include <cstdint>
#include <sstream>
#include <string>
#include <vector>

#include "logging.h"
#include "output_renderer.h"
#include "platform.h"
#include "process_session.h"
#include "session_pump.h"
#include "session_pump_internal.h"
#include "session_response_builder.h"
#include "session_store.h"

namespace {

std::atomic<unsigned long> next_id(1UL);
std::atomic<std::uint64_t> next_touch_order(1ULL);

std::string make_exec_session_id() {
    std::ostringstream out;
    out << "sess_" << platform::monotonic_ms() << "_" << next_id.fetch_add(1UL);
    return out.str();
}

std::uint64_t make_touch_order() {
    return next_touch_order.fetch_add(1ULL);
}

struct PollResult {
    std::string output;
    bool completed;
    int exit_code;
};

const unsigned long EXIT_POLL_INTERVAL_MS = 25UL;
const unsigned long RECENT_PROTECTION_COUNT = 8UL;
const unsigned long WARNING_THRESHOLD_HEADROOM = 4UL;
const unsigned long WARNING_THRESHOLD = DEFAULT_MAX_OPEN_SESSIONS - WARNING_THRESHOLD_HEADROOM;

struct PruneCandidate {
    std::string daemon_session_id;
    std::shared_ptr<LiveSession> session;
    std::uint64_t last_touched_order;
};

struct SessionSnapshot {
    std::string buffered_output;
    bool eof;
    bool exited;
    int exit_code;
    std::uint64_t generation;
};

Json empty_exec_warnings() {
    return Json::array();
}

Json session_limit_warning(const std::string& target, unsigned long open_sessions) {
    return Json::array({Json{
        {"code", "exec_session_limit_approaching"},
        {"message", "Target `" + target + "` now has " + std::to_string(open_sessions) + " open exec sessions."},
    }});
}

bool crosses_warning_threshold(std::size_t open_sessions) {
    return open_sessions < WARNING_THRESHOLD && open_sessions + 1U >= WARNING_THRESHOLD;
}

std::size_t protected_recent_count(std::size_t open_sessions) {
    if (open_sessions == 0U) {
        return 0U;
    }
    return std::min<std::size_t>(RECENT_PROTECTION_COUNT, open_sessions - 1U);
}

std::shared_ptr<LiveSession> launch_live_session(
    const std::string& command, const std::string& workdir, const std::string& shell, bool login, bool tty) {
    std::shared_ptr<LiveSession> session(new LiveSession());
    session->id = make_exec_session_id();
    session->process = ProcessSession::launch(command, workdir, shell, login, tty);
    session->started_at_ms = platform::monotonic_ms();
#ifdef _WIN32
    session->stdin_open = true;
#else
    session->stdin_open = tty;
#endif
    return session;
}

SessionSnapshot session_snapshot_locked(const LiveSession& session) {
    SessionSnapshot snapshot;
    snapshot.buffered_output = session.output_.buffered_output;
    snapshot.eof = session.output_.eof;
    snapshot.exited = session.output_.exited;
    snapshot.exit_code = session.output_.exit_code;
    snapshot.generation = session.output_.generation;
    return snapshot;
}

void wait_for_generation_change_locked(LiveSession* session,
                                       std::uint64_t baseline_generation,
                                       std::uint64_t deadline_ms,
                                       unsigned long max_wait_ms) {
    while (!session->closing && session->output_.generation == baseline_generation) {
        const std::uint64_t now = platform::monotonic_ms();
        if (now >= deadline_ms) {
            return;
        }
        unsigned long remaining = static_cast<unsigned long>(deadline_ms - now);
        if (max_wait_ms > 0UL) {
            remaining = std::min(remaining, max_wait_ms);
        }
        if (!session->cond_.timed_wait_ms(session->mutex_, remaining)) {
            return;
        }
    }
}

PollResult wait_for_session_activity(const std::shared_ptr<LiveSession>& session,
                                     unsigned long timeout_ms,
                                     unsigned long max_output_tokens) {
    const std::uint64_t deadline = platform::monotonic_ms() + timeout_ms;
    BasicLockGuard session_lock(session->mutex_);
    std::string output;
    std::uint64_t seen_generation = session->output_.generation;

    for (;;) {
        mark_session_exit_locked(session.get());
        const SessionSnapshot snapshot = session_snapshot_locked(*session);
        if (!snapshot.buffered_output.empty()) {
            output += take_session_output_locked(session.get(), max_output_tokens);
            seen_generation = session->output_.generation;
        }
        if (snapshot.exited) {
            if (!drain_exited_session_output_locked(session.get(), &output, max_output_tokens)) {
                return PollResult{output, false, 0};
            }
            const SessionSnapshot completed = session_snapshot_locked(*session);
            return PollResult{output, true, completed.exit_code};
        }

        const std::uint64_t now = platform::monotonic_ms();
        if (timeout_ms == 0UL || now >= deadline) {
            return PollResult{output, false, 0};
        }

        wait_for_generation_change_locked(session.get(), seen_generation, deadline, EXIT_POLL_INTERVAL_MS);
    }
}

void retire_session(const std::shared_ptr<LiveSession>& session) {
    BasicLockGuard session_lock(session->mutex_);
    session->retired = true;
    session->closing = true;
    session->cond_.broadcast();
    if (session->process.get() != NULL) {
        session->process->terminate();
    }
}

} // namespace

SessionOutputState::SessionOutputState() : eof(false), exited(false), exit_code(0), generation(0) {
}

LiveSession::LiveSession()
    : started_at_ms(0), last_touched_order(0), stdin_open(false), retired(false), closing(false), pump_started(false)
#ifdef _WIN32
      ,
      pump_thread_(NULL)
#endif
{
}

LiveSession::~LiveSession() {
}

SessionStore::SessionStore() : pending_starts_(0UL) {
}

SessionStore::~SessionStore() {
    std::vector<std::shared_ptr<LiveSession>> sessions;
    {
        BasicLockGuard lock(mutex_);
        for (std::map<std::string, std::shared_ptr<LiveSession>>::iterator it = sessions_.begin();
             it != sessions_.end();
             ++it) {
            sessions.push_back(it->second);
        }
        sessions_.clear();
        pending_starts_ = 0UL;
    }

    for (std::size_t i = 0; i < sessions.size(); ++i) {
        retire_session(sessions[i]);
        join_session_pump(sessions[i].get());
    }
}

class PendingStartReservation {
public:
    PendingStartReservation(BasicMutex& mutex, unsigned long* pending_starts)
        : mutex_(mutex), pending_starts_(pending_starts), active_(true) {}

    ~PendingStartReservation() { release(); }

    void release() {
        if (!active_) {
            return;
        }
        BasicLockGuard lock(mutex_);
        release_locked();
    }

    void release_locked() {
        if (!active_) {
            return;
        }
        if (*pending_starts_ > 0UL) {
            --(*pending_starts_);
        }
        active_ = false;
    }

private:
    BasicMutex& mutex_;
    unsigned long* pending_starts_;
    bool active_;

    PendingStartReservation(const PendingStartReservation&);
    PendingStartReservation& operator=(const PendingStartReservation&);
};

void erase_session_if_current(BasicMutex& mutex,
                              std::map<std::string, std::shared_ptr<LiveSession>>& sessions,
                              const std::string& daemon_session_id,
                              const std::shared_ptr<LiveSession>& session) {
    BasicLockGuard lock(mutex);
    std::map<std::string, std::shared_ptr<LiveSession>>::iterator it = sessions.find(daemon_session_id);
    if (it != sessions.end() && it->second == session) {
        sessions.erase(it);
    }
}

bool remove_session_if_current(BasicMutex& mutex,
                               std::map<std::string, std::shared_ptr<LiveSession>>& sessions,
                               const std::string& daemon_session_id,
                               const std::shared_ptr<LiveSession>& session,
                               std::shared_ptr<LiveSession>* removed) {
    BasicLockGuard lock(mutex);
    std::map<std::string, std::shared_ptr<LiveSession>>::iterator it = sessions.find(daemon_session_id);
    if (it == sessions.end() || it->second != session) {
        return false;
    }
    *removed = it->second;
    sessions.erase(it);
    return true;
}

bool SessionStore::reserve_pending_start(unsigned long max_open_sessions) {
    for (;;) {
        {
            BasicLockGuard lock(mutex_);
            if (sessions_.size() + pending_starts_ < max_open_sessions) {
                ++pending_starts_;
                return true;
            }
        }

        if (!prune_one_session_for_start(max_open_sessions)) {
            return false;
        }
    }
}

bool SessionStore::prune_one_session_for_start(unsigned long max_open_sessions) {
    for (;;) {
        std::vector<PruneCandidate> snapshot;
        {
            BasicLockGuard lock(mutex_);
            if (sessions_.size() + pending_starts_ < max_open_sessions) {
                return true;
            }
            if (sessions_.empty()) {
                return false;
            }
            snapshot.reserve(sessions_.size());
            for (std::map<std::string, std::shared_ptr<LiveSession>>::const_iterator it = sessions_.begin();
                 it != sessions_.end();
                 ++it) {
                snapshot.push_back(PruneCandidate{
                    it->first,
                    it->second,
                    it->second->last_touched_order.load(),
                });
            }
        }

        std::sort(snapshot.begin(), snapshot.end(), [](const PruneCandidate& left, const PruneCandidate& right) {
            if (left.last_touched_order != right.last_touched_order) {
                return left.last_touched_order < right.last_touched_order;
            }
            return left.daemon_session_id < right.daemon_session_id;
        });

        const std::size_t prunable_count = snapshot.size() - protected_recent_count(snapshot.size());
        if (prunable_count == 0U) {
            return false;
        }

        PruneCandidate victim = snapshot[0];
        bool found_exited = false;
        for (std::size_t i = 0; i < prunable_count; ++i) {
            BasicLockGuard session_lock(snapshot[i].session->mutex_);
            int exit_code = 0;
            if (snapshot[i].session->retired || snapshot[i].session->output_.exited ||
                snapshot[i].session->process->has_exited(&exit_code)) {
                victim = snapshot[i];
                found_exited = true;
                break;
            }
        }
        if (!found_exited) {
            victim = snapshot[0];
        }

        std::shared_ptr<LiveSession> removed;
        if (!remove_session_if_current(mutex_, sessions_, victim.daemon_session_id, victim.session, &removed)) {
            continue;
        }

        retire_session(removed);
        join_session_pump(removed.get());

        unsigned long open_sessions_after_prune = 0UL;
        {
            BasicLockGuard lock(mutex_);
            open_sessions_after_prune = static_cast<unsigned long>(sessions_.size());
        }
        LogMessageBuilder message("pruned exec session");
        message.quoted_field("daemon_session_id", victim.daemon_session_id)
            .field("open_sessions", open_sessions_after_prune);
        log_message(LOG_WARN, "session_store", message.str());
        return true;
    }
}

Json SessionStore::start_command(const std::string& target,
                                 const std::string& command,
                                 const std::string& workdir,
                                 const std::string& shell,
                                 bool login,
                                 bool tty,
                                 bool has_yield_time_ms,
                                 unsigned long yield_time_ms,
                                 unsigned long max_output_tokens,
                                 const YieldTimeConfig& yield_time,
                                 unsigned long max_open_sessions) {
    if (!reserve_pending_start(max_open_sessions)) {
        throw SessionLimitError("too many open exec sessions");
    }
    PendingStartReservation pending_start(mutex_, &pending_starts_);

    {
        LogMessageBuilder message("start_command");
        message.quoted_field("cmd_preview", preview_text(command, 120))
            .quoted_field("workdir", workdir)
            .quoted_field("shell", shell)
            .bool_field("login", login)
            .bool_field("tty", tty);
        log_message(LOG_INFO, "session_store", message.str());
    }
    std::shared_ptr<LiveSession> session = launch_live_session(command, workdir, shell, login, tty);
    start_session_pump(session);

    const unsigned long timeout_ms = resolve_yield_time_ms(yield_time.exec_command, has_yield_time_ms, yield_time_ms);
    BasicLockGuard operation_lock(session->operation_mutex_);
    const PollResult poll_result = wait_for_session_activity(session, timeout_ms, max_output_tokens);

    Json warnings = empty_exec_warnings();
    {
        BasicLockGuard lock(mutex_);
        pending_start.release_locked();
        if (!poll_result.completed) {
            const bool crossed_warning_threshold = crosses_warning_threshold(sessions_.size());
            session->last_touched_order.store(make_touch_order());
            sessions_[session->id] = session;
            LogMessageBuilder message("stored live session");
            message.quoted_field("daemon_session_id", session->id).field("open_sessions", sessions_.size());
            log_message(LOG_INFO, "session_store", message.str());
            if (crossed_warning_threshold) {
                warnings = session_limit_warning(target, WARNING_THRESHOLD);
            }
        }
    }

    if (poll_result.completed) {
        retire_session(session);
        join_session_pump(session.get());
        Json response = build_session_response(NULL,
                                               false,
                                               session->started_at_ms,
                                               true,
                                               poll_result.exit_code,
                                               poll_result.output,
                                               max_output_tokens,
                                               empty_exec_warnings());
        {
            LogMessageBuilder message("command completed before session handoff");
            message.field("exit_code", poll_result.exit_code).field("output_chars", poll_result.output.size());
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
    }

    return build_session_response(
        session->id.c_str(), true, session->started_at_ms, false, 0, poll_result.output, max_output_tokens, warnings);
}

Json SessionStore::write_stdin(const std::string& daemon_session_id,
                               const std::string& chars,
                               bool has_yield_time_ms,
                               unsigned long yield_time_ms,
                               unsigned long max_output_tokens,
                               const YieldTimeConfig& yield_time,
                               bool has_pty_size,
                               unsigned short pty_rows,
                               unsigned short pty_cols) {
    std::shared_ptr<LiveSession> session;
    {
        BasicLockGuard lock(mutex_);
        std::map<std::string, std::shared_ptr<LiveSession>>::iterator it = sessions_.find(daemon_session_id);
        if (it == sessions_.end()) {
            log_message(LOG_WARN, "session_store", "unknown daemon session `" + daemon_session_id + "`");
            throw UnknownSessionError("Unknown daemon session");
        }
        session = it->second;
        session->last_touched_order.store(make_touch_order());
    }

    {
        LogMessageBuilder message("write_stdin");
        message.quoted_field("daemon_session_id", daemon_session_id).field("chars_len", chars.size());
        log_message(LOG_INFO, "session_store", message.str());
    }

    PollResult poll_result;
    {
        BasicLockGuard operation_lock(session->operation_mutex_);
        {
            BasicLockGuard session_lock(session->mutex_);
            if (session->retired) {
                log_message(LOG_WARN, "session_store", "unknown daemon session `" + daemon_session_id + "`");
                throw UnknownSessionError("Unknown daemon session");
            }

            if (has_pty_size) {
                if (pty_rows == 0U || pty_cols == 0U) {
                    throw ProcessPtyResizeUnsupportedError("PTY rows and cols must be greater than zero");
                }
                session->process->resize_pty(pty_rows, pty_cols);
            }

            if (!chars.empty()) {
                if (!session->stdin_open) {
                    throw StdinClosedError(
                        "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open");
                }
                try {
                    session->process->write_stdin(chars);
                } catch (const ProcessStdinClosedError& ex) {
                    session->stdin_open = false;
                    throw StdinClosedError(ex.what());
                }
            }
        }
        const YieldTimeOperationConfig& operation_config =
            chars.empty() ? yield_time.write_stdin_poll : yield_time.write_stdin_input;
        const unsigned long timeout_ms = resolve_yield_time_ms(operation_config, has_yield_time_ms, yield_time_ms);
        poll_result = wait_for_session_activity(session, timeout_ms, max_output_tokens);
    }

    if (poll_result.completed) {
        retire_session(session);
        erase_session_if_current(mutex_, sessions_, daemon_session_id, session);
        join_session_pump(session.get());
        Json response = build_session_response(NULL,
                                               false,
                                               session->started_at_ms,
                                               true,
                                               poll_result.exit_code,
                                               poll_result.output,
                                               max_output_tokens,
                                               empty_exec_warnings());
        {
            unsigned long open_sessions = 0UL;
            {
                BasicLockGuard lock(mutex_);
                open_sessions = static_cast<unsigned long>(sessions_.size() + pending_starts_);
            }
            LogMessageBuilder message("session completed");
            message.quoted_field("daemon_session_id", daemon_session_id)
                .field("exit_code", poll_result.exit_code)
                .field("open_sessions", open_sessions);
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
    }

    {
        LogMessageBuilder message("session still running");
        message.quoted_field("daemon_session_id", session->id);
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_session_response(session->id.c_str(),
                                  true,
                                  session->started_at_ms,
                                  false,
                                  0,
                                  poll_result.output,
                                  max_output_tokens,
                                  empty_exec_warnings());
}
