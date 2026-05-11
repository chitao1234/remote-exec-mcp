#include <algorithm>
#include <atomic>
#include <cstdint>
#include <sstream>
#include <string>
#include <vector>

#include "logging.h"
#include "platform.h"
#include "process_session.h"
#include "session_store.h"
#ifdef _WIN32
#include "win32_thread.h"
#endif

namespace {

std::atomic<unsigned long> next_id(1UL);
std::atomic<std::uint64_t> next_touch_order(1ULL);

std::string make_chunk_id() {
    std::ostringstream out;
    out << platform::monotonic_ms() << '-' << next_id.fetch_add(1UL);
    return out.str();
}

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

const unsigned long EXIT_DRAIN_INITIAL_WAIT_MS = 125UL;
const unsigned long EXIT_DRAIN_QUIET_MS = 25UL;
const unsigned long EXIT_POLL_INTERVAL_MS = 25UL;
const unsigned long RECENT_PROTECTION_COUNT = 8UL;
const unsigned long WARNING_THRESHOLD = 60UL;

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

const std::size_t BYTES_PER_TOKEN = 4U;

unsigned long approximate_token_count(std::size_t bytes) {
    if (bytes == 0U) {
        return 0UL;
    }
    return static_cast<unsigned long>((bytes + BYTES_PER_TOKEN - 1U) / BYTES_PER_TOKEN);
}

unsigned long count_lines(const std::string& output) {
    if (output.empty()) {
        return 0UL;
    }

    unsigned long lines = 1UL;
    for (std::string::const_iterator it = output.begin(); it != output.end(); ++it) {
        if (*it == '\n') {
            ++lines;
        }
    }
    if (output[output.size() - 1] == '\n') {
        --lines;
    }
    return lines;
}

bool is_utf8_continuation_byte(unsigned char byte) {
    return (byte & 0xC0U) == 0x80U;
}

std::size_t floor_char_boundary(const std::string& output, std::size_t max_bytes) {
    std::size_t index = std::min(max_bytes, output.size());
    while (index > 0U && index < output.size() &&
           is_utf8_continuation_byte(static_cast<unsigned char>(output[index]))) {
        --index;
    }
    return index;
}

std::size_t ceil_char_boundary(const std::string& output, std::size_t min_bytes) {
    std::size_t index = std::min(min_bytes, output.size());
    while (index < output.size() &&
           is_utf8_continuation_byte(static_cast<unsigned char>(output[index]))) {
        ++index;
    }
    return index;
}

std::size_t suffix_start_for_budget(const std::string& output, std::size_t max_bytes) {
    if (max_bytes >= output.size()) {
        return 0U;
    }
    return ceil_char_boundary(output, output.size() - max_bytes);
}

std::string truncation_prefix(unsigned long line_count) {
    std::ostringstream out;
    out << "Total output lines: " << line_count << "\n\n";
    return out.str();
}

std::string truncation_marker(unsigned long truncated_tokens) {
    std::ostringstream out;
    out << "\xE2\x80\xA6" << truncated_tokens << " tokens truncated" << "\xE2\x80\xA6";
    return out.str();
}

std::string render_output(const std::string& output, unsigned long max_output_tokens) {
    if (max_output_tokens == 0UL) {
        return std::string();
    }

    const std::size_t max_output_bytes =
        static_cast<std::size_t>(max_output_tokens) * BYTES_PER_TOKEN;
    if (output.size() <= max_output_bytes) {
        return output;
    }

    const std::string prefix = truncation_prefix(count_lines(output));
    unsigned long truncated_tokens = approximate_token_count(output.size());
    for (;;) {
        const std::string marker = truncation_marker(truncated_tokens);
        if (max_output_bytes <= prefix.size() + marker.size()) {
            return prefix + marker;
        }

        const std::size_t payload_budget = max_output_bytes - prefix.size() - marker.size();
        const std::size_t head_budget = payload_budget / 2U;
        const std::size_t tail_budget = payload_budget - head_budget;
        const std::size_t head_end = floor_char_boundary(output, head_budget);
        const std::size_t tail_start =
            std::max(head_end, suffix_start_for_budget(output, tail_budget));
        const unsigned long next_truncated_tokens =
            approximate_token_count(tail_start - head_end);

        if (next_truncated_tokens == truncated_tokens) {
            return prefix + output.substr(0, head_end) + marker + output.substr(tail_start);
        }

        truncated_tokens = next_truncated_tokens;
    }
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
    unsigned long max_output_tokens,
    const Json& warnings
) {
    const std::string trimmed = render_output(output, max_output_tokens);
    const unsigned long original_token_count = approximate_token_count(output.size());
    return Json{
        {"daemon_session_id", daemon_session_id != NULL ? Json(daemon_session_id) : Json(nullptr)},
        {"running", running},
        {"chunk_id", make_chunk_id()},
        {"wall_time_seconds", wall_time_seconds(started_at_ms)},
        {"exit_code", has_exit_code ? Json(exit_code) : Json(nullptr)},
        {"original_token_count", original_token_count},
        {"output", trimmed},
        {"warnings", warnings}
    };
}

Json empty_exec_warnings() {
    return Json::array();
}

Json session_limit_warning(const std::string& target) {
    return Json::array(
        {Json{
            {"code", "exec_session_limit_approaching"},
            {"message", "Target `" + target + "` now has 60 open exec sessions."},
        }}
    );
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
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login,
    bool tty
) {
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

void wait_for_generation_change_locked(
    LiveSession* session,
    std::uint64_t baseline_generation,
    std::uint64_t deadline_ms,
    unsigned long max_wait_ms
) {
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

void append_session_output_locked(
    LiveSession* session,
    const std::string& chunk
) {
    if (!chunk.empty()) {
        session->output_.buffered_output += chunk;
        ++session->output_.generation;
        session->cond_.broadcast();
    }
}

bool mark_session_exit_locked(LiveSession* session) {
    if (session->output_.exited) {
        return false;
    }
    int exit_code = session->output_.exit_code;
    if (session->process->has_exited(&exit_code)) {
        session->output_.exited = true;
        session->output_.exit_code = exit_code;
        ++session->output_.generation;
        session->cond_.broadcast();
        return true;
    }
    return false;
}

void finish_session_output_locked(LiveSession* session) {
    const std::string flushed = session->process->flush_carry(&session->output_.decode_carry);
    if (!flushed.empty()) {
        session->output_.buffered_output += flushed;
    }
    session->output_.eof = true;
    mark_session_exit_locked(session);
    ++session->output_.generation;
    session->cond_.broadcast();
}

bool terminate_descendants_after_exit_locked(LiveSession* session) {
    if (session->process.get() != NULL) {
        return session->process->terminate_descendants();
    }
    return false;
}

void pump_session_output(const std::shared_ptr<LiveSession>& session) {
    for (;;) {
        {
            BasicLockGuard lock(session->mutex_);
            if (session->closing || session->retired || session->process.get() == NULL) {
                return;
            }
        }

        bool eof = false;
        std::string carry;
        std::string chunk;
        {
            BasicLockGuard lock(session->mutex_);
            carry = session->output_.decode_carry;
        }

        try {
            chunk = session->process->read_output(true, &eof, &carry);
        } catch (const std::exception&) {
            BasicLockGuard lock(session->mutex_);
            session->output_.decode_carry = carry;
            finish_session_output_locked(session.get());
            session->retired = true;
            return;
        }

        BasicLockGuard lock(session->mutex_);
        if (session->closing) {
            return;
        }
        session->output_.decode_carry = carry;
        append_session_output_locked(session.get(), chunk);
        if (eof) {
            finish_session_output_locked(session.get());
            return;
        }
    }
}

#ifdef _WIN32
struct SessionPumpContext {
    std::shared_ptr<LiveSession> session;
};

unsigned __stdcall session_output_pump_entry(void* raw_context) {
    std::unique_ptr<SessionPumpContext> context(
        static_cast<SessionPumpContext*>(raw_context)
    );
    pump_session_output(context->session);
    return 0;
}

void start_session_pump(const std::shared_ptr<LiveSession>& session) {
    BasicLockGuard lock(session->mutex_);
    if (session->pump_started) {
        return;
    }
    std::unique_ptr<SessionPumpContext> context(new SessionPumpContext());
    context->session = session;
    HANDLE handle = begin_win32_thread(session_output_pump_entry, context.get());
    if (handle == NULL) {
        throw std::runtime_error("_beginthreadex failed");
    }
    session->pump_thread_ = handle;
    session->pump_started = true;
    context.release();
}

void join_session_pump(LiveSession* session) {
    HANDLE handle = NULL;
    {
        BasicLockGuard lock(session->mutex_);
        handle = session->pump_thread_;
        session->pump_thread_ = NULL;
        session->pump_started = false;
    }
    if (handle != NULL) {
        WaitForSingleObject(handle, INFINITE);
        CloseHandle(handle);
    }
}
#else
void start_session_pump(const std::shared_ptr<LiveSession>& session) {
    BasicLockGuard lock(session->mutex_);
    if (session->pump_started) {
        return;
    }
    session->pump_thread_.reset(new std::thread(pump_session_output, session));
    session->pump_started = true;
}

void join_session_pump(LiveSession* session) {
    std::unique_ptr<std::thread> thread;
    {
        BasicLockGuard lock(session->mutex_);
        thread.swap(session->pump_thread_);
        session->pump_started = false;
    }
    if (thread.get() != NULL) {
        thread->join();
    }
}
#endif

std::string take_session_output_locked(
    LiveSession* session,
    unsigned long max_output_tokens
) {
    (void)max_output_tokens;
    std::string output = session->output_.buffered_output;
    session->output_.buffered_output.clear();
    return output;
}

bool drain_exited_session_output_locked(
    LiveSession* session,
    std::string* output,
    unsigned long max_output_tokens
) {
    bool signaled_descendants = false;
    std::uint64_t deadline = platform::monotonic_ms() + EXIT_DRAIN_INITIAL_WAIT_MS;

    for (;;) {
        if (!session->output_.buffered_output.empty()) {
            *output += take_session_output_locked(session, max_output_tokens);
        }
        if (session->output_.eof || session->closing) {
            return true;
        }

        const std::uint64_t now = platform::monotonic_ms();
        if (now >= deadline) {
            if (signaled_descendants) {
                return session->output_.eof || session->closing;
            }
            if (!terminate_descendants_after_exit_locked(session)) {
                return false;
            }
            signaled_descendants = true;
            deadline = platform::monotonic_ms() + EXIT_DRAIN_QUIET_MS;
            continue;
        }

        const std::uint64_t seen_generation = session->output_.generation;
        wait_for_generation_change_locked(session, seen_generation, deadline, 0UL);
    }
}

PollResult wait_for_session_activity(
    const std::shared_ptr<LiveSession>& session,
    unsigned long timeout_ms,
    unsigned long max_output_tokens
) {
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
            if (!drain_exited_session_output_locked(
                    session.get(),
                    &output,
                    max_output_tokens
                )) {
                return PollResult{output, false, 0};
            }
            const SessionSnapshot completed = session_snapshot_locked(*session);
            return PollResult{output, true, completed.exit_code};
        }

        const std::uint64_t now = platform::monotonic_ms();
        if (timeout_ms == 0UL || now >= deadline) {
            return PollResult{output, false, 0};
        }

        wait_for_generation_change_locked(
            session.get(),
            seen_generation,
            deadline,
            EXIT_POLL_INTERVAL_MS
        );
    }
}

}  // namespace

SessionOutputState::SessionOutputState()
    : eof(false), exited(false), exit_code(0), generation(0) {}

LiveSession::LiveSession()
    : started_at_ms(0),
      last_touched_order(0),
      stdin_open(false),
      retired(false),
      closing(false),
      pump_started(false)
#ifdef _WIN32
      ,
      pump_thread_(NULL)
#endif
{
}

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
        {
            BasicLockGuard session_lock(sessions[i]->mutex_);
            sessions[i]->retired = true;
            sessions[i]->closing = true;
            sessions[i]->cond_.broadcast();
            if (sessions[i]->process.get() != NULL) {
                sessions[i]->process->terminate();
            }
        }
        join_session_pump(sessions[i].get());
    }
}

class PendingStartReservation {
public:
    PendingStartReservation(BasicMutex& mutex, unsigned long* pending_starts)
        : mutex_(mutex), pending_starts_(pending_starts), active_(true) {}

    ~PendingStartReservation() {
        release();
    }

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

bool remove_session_if_current(
    BasicMutex& mutex,
    std::map<std::string, std::shared_ptr<LiveSession> >& sessions,
    const std::string& daemon_session_id,
    const std::shared_ptr<LiveSession>& session,
    std::shared_ptr<LiveSession>* removed
) {
    BasicLockGuard lock(mutex);
    std::map<std::string, std::shared_ptr<LiveSession> >::iterator it =
        sessions.find(daemon_session_id);
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
            for (std::map<std::string, std::shared_ptr<LiveSession> >::const_iterator it =
                     sessions_.begin();
                 it != sessions_.end();
                 ++it) {
                snapshot.push_back(
                    PruneCandidate{
                        it->first,
                        it->second,
                        it->second->last_touched_order.load(),
                    }
                );
            }
        }

        std::sort(
            snapshot.begin(),
            snapshot.end(),
            [](const PruneCandidate& left, const PruneCandidate& right) {
                if (left.last_touched_order != right.last_touched_order) {
                    return left.last_touched_order < right.last_touched_order;
                }
                return left.daemon_session_id < right.daemon_session_id;
            }
        );

        const std::size_t prunable_count =
            snapshot.size() - protected_recent_count(snapshot.size());
        if (prunable_count == 0U) {
            return false;
        }

        PruneCandidate victim = snapshot[0];
        bool found_exited = false;
        for (std::size_t i = 0; i < prunable_count; ++i) {
            BasicLockGuard session_lock(snapshot[i].session->mutex_);
            int exit_code = 0;
            if (snapshot[i].session->retired ||
                snapshot[i].session->output_.exited ||
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
        if (!remove_session_if_current(
                mutex_,
                sessions_,
                victim.daemon_session_id,
                victim.session,
                &removed
            )) {
            continue;
        }

        {
            BasicLockGuard session_lock(removed->mutex_);
            removed->retired = true;
            removed->closing = true;
            removed->cond_.broadcast();
            if (removed->process.get() != NULL) {
                removed->process->terminate();
            }
        }
        join_session_pump(removed.get());

        unsigned long open_sessions_after_prune = 0UL;
        {
            BasicLockGuard lock(mutex_);
            open_sessions_after_prune = static_cast<unsigned long>(sessions_.size());
        }
        std::ostringstream message;
        message << "pruned exec session daemon_session_id=`" << victim.daemon_session_id
                << "` open_sessions=" << open_sessions_after_prune;
        log_message(LOG_WARN, "session_store", message.str());
        return true;
    }
}

Json SessionStore::start_command(
    const std::string& target,
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
    if (!reserve_pending_start(max_open_sessions)) {
        throw SessionLimitError("too many open exec sessions");
    }
    PendingStartReservation pending_start(mutex_, &pending_starts_);

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
    start_session_pump(session);

    const unsigned long timeout_ms = resolve_yield_time_ms(
        yield_time.exec_command,
        has_yield_time_ms,
        yield_time_ms
    );
    BasicLockGuard operation_lock(session->operation_mutex_);
    const PollResult poll_result =
        wait_for_session_activity(session, timeout_ms, max_output_tokens);

    Json warnings = empty_exec_warnings();
    {
        BasicLockGuard lock(mutex_);
        pending_start.release_locked();
        if (!poll_result.completed) {
            const bool crossed_warning_threshold = crosses_warning_threshold(sessions_.size());
            session->last_touched_order.store(make_touch_order());
            sessions_[session->id] = session;
            std::ostringstream message;
            message << "stored live session daemon_session_id=`" << session->id
                    << "` open_sessions=" << sessions_.size();
            log_message(LOG_INFO, "session_store", message.str());
            if (crossed_warning_threshold) {
                warnings = session_limit_warning(target);
            }
        }
    }

    if (poll_result.completed) {
        {
            BasicLockGuard session_lock(session->mutex_);
            session->retired = true;
            session->closing = true;
            session->cond_.broadcast();
            if (session->process.get() != NULL) {
                session->process->terminate();
            }
        }
        join_session_pump(session.get());
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_tokens,
            empty_exec_warnings()
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
        max_output_tokens,
        warnings
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
        session->last_touched_order.store(make_touch_order());
    }

    {
        std::ostringstream message;
        message << "write_stdin daemon_session_id=`" << daemon_session_id
                << "` chars_len=" << chars.size();
        log_message(LOG_INFO, "session_store", message.str());
    }

    PollResult poll_result;
    {
        BasicLockGuard operation_lock(session->operation_mutex_);
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
        const unsigned long timeout_ms = resolve_yield_time_ms(
            operation_config,
            has_yield_time_ms,
            yield_time_ms
        );
        poll_result = wait_for_session_activity(session, timeout_ms, max_output_tokens);
    }

    if (poll_result.completed) {
        {
            BasicLockGuard session_lock(session->mutex_);
            session->retired = true;
            session->closing = true;
            session->cond_.broadcast();
            if (session->process.get() != NULL) {
                session->process->terminate();
            }
        }
        erase_session_if_current(mutex_, sessions_, daemon_session_id, session);
        join_session_pump(session.get());
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_tokens,
            empty_exec_warnings()
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
        max_output_tokens,
        empty_exec_warnings()
    );
}
