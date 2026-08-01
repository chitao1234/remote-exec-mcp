#include "exec/session_pump.h"

#include <algorithm>
#include <limits>
#include <stdexcept>
#include <string>
#include <thread>

#include "core/logging.h"
#include "exec/process_session.h"
#include "platform/deadline.h"
#include "platform/platform.h"
#include "runtime/daemon_thread.h"
#include "session_pump_internal.h"

namespace {

const unsigned long DEFAULT_EXIT_DRAIN_IDLE_GRACE_MS = 250UL;
const unsigned long DEFAULT_EXIT_DRAIN_MAX_GRACE_MS = 2000UL;
const unsigned long DEFAULT_EXIT_DRAIN_TERMINATE_QUIET_MS = 25UL;

std::uint64_t deadline_after_ms(std::uint64_t started_at_ms, unsigned long timeout_ms) {
    const std::uint64_t timeout = static_cast<std::uint64_t>(timeout_ms);
    const std::uint64_t max_deadline = std::numeric_limits<std::uint64_t>::max();
    return max_deadline - started_at_ms < timeout ? max_deadline : started_at_ms + timeout;
}

void record_session_drain_stop_locked(LiveSession* session, SessionOutputDrainStopReason reason) {
    session->output_.last_drain_stop_reason = reason;
}

SessionOutputDrainResult make_session_drain_result_locked(
    LiveSession* session,
    bool completed,
    SessionOutputDrainStopReason reason
) {
    record_session_drain_stop_locked(session, reason);
    return SessionOutputDrainResult(completed, reason);
}

void ensure_session_drain_started_locked(LiveSession* session) {
    if (session->output_.drain_started) {
        return;
    }
    const std::uint64_t now = platform::monotonic_ms();
    session->output_.drain_started = true;
    session->output_.drain_started_at_ms = now;
    session->output_.drain_last_output_at_ms = now;
}

void note_session_drain_output_locked(LiveSession* session) {
    if (!session->output_.drain_started) {
        ensure_session_drain_started_locked(session);
    }
    session->output_.drain_last_output_at_ms = platform::monotonic_ms();
    record_session_drain_stop_locked(session, SessionOutputDrainStopReason::None);
}

SessionOutputDrainStopReason grace_deadline_stop_reason_locked(
    LiveSession* session,
    const SessionOutputDrainPolicy& policy,
    std::uint64_t* next_deadline_ms
) {
    const std::uint64_t max_deadline =
        deadline_after_ms(session->output_.drain_started_at_ms, policy.max_grace_ms);
    const std::uint64_t idle_deadline =
        deadline_after_ms(session->output_.drain_last_output_at_ms, policy.idle_grace_ms);

    if (platform::monotonic_deadline_expired(max_deadline)) {
        return SessionOutputDrainStopReason::MaxGraceExpired;
    }
    if (platform::monotonic_deadline_expired(idle_deadline)) {
        return SessionOutputDrainStopReason::IdleGraceExpired;
    }

    *next_deadline_ms = std::min(max_deadline, idle_deadline);
    return SessionOutputDrainStopReason::None;
}

SessionOutputDrainStopReason eof_stop_reason_locked(const LiveSession* session) {
    if (session->output_.last_drain_stop_reason == SessionOutputDrainStopReason::PumpError) {
        return SessionOutputDrainStopReason::PumpError;
    }
    return SessionOutputDrainStopReason::OutputEof;
}

void append_session_output_locked(LiveSession* session, const std::string& chunk) {
    if (!chunk.empty()) {
        session->output_.buffered_output += chunk;
        if (session->output_.exited) {
            note_session_drain_output_locked(session);
        }
        ++session->output_.generation;
        session->cond_.broadcast();
    }
}

bool terminate_descendants_after_exit_locked(LiveSession* session) {
    if (session->process.get() != nullptr) {
        return session->process->terminate_descendants();
    }
    return false;
}

void pump_session_output(const std::shared_ptr<LiveSession>& session) {
    for (;;) {
        {
            BasicLockGuard lock(session->mutex_);
            if (session->closing || session->retired || session->process.get() == nullptr) {
                return;
            }
        }

        bool eof = false;
        std::string carry;
        std::string chunk;
        {
            // Move the carry out under lock. The lock must be released before
            // read_output because it is a blocking I/O call. The carry is
            // moved back under lock after the call completes or throws.
            BasicLockGuard lock(session->mutex_);
            carry = std::move(session->output_.decode_carry);
        }

        try {
            chunk = session->process->read_output(true, &eof, &carry);
        } catch (const std::exception& ex) {
            log_message(
                LOG_WARN,
                "session",
                std::string("session output pump failed: ") + ex.what()
            );
            BasicLockGuard lock(session->mutex_);
            session->output_.decode_carry = std::move(carry);
            finish_session_output_locked(session.get(), SessionOutputDrainStopReason::PumpError);
            session->retired = true;
            return;
        }

        BasicLockGuard lock(session->mutex_);
        session->output_.decode_carry = std::move(carry);
        if (session->closing) {
            return;
        }
        append_session_output_locked(session.get(), chunk);
        if (eof) {
            finish_session_output_locked(session.get(), SessionOutputDrainStopReason::OutputEof);
            return;
        }
    }
}

} // namespace

const char* session_output_drain_stop_reason_name(SessionOutputDrainStopReason reason) {
    switch (reason) {
    case SessionOutputDrainStopReason::None:
        return "none";
    case SessionOutputDrainStopReason::OutputEof:
        return "output_eof";
    case SessionOutputDrainStopReason::StoreClosing:
        return "store_closing";
    case SessionOutputDrainStopReason::PumpError:
        return "pump_error";
    case SessionOutputDrainStopReason::IdleGraceExpired:
        return "idle_grace_expired";
    case SessionOutputDrainStopReason::MaxGraceExpired:
        return "max_grace_expired";
    case SessionOutputDrainStopReason::DescendantTerminateUnsupported:
        return "descendant_terminate_unsupported";
    case SessionOutputDrainStopReason::DescendantTerminateTimeout:
        return "descendant_terminate_timeout";
    }
    return "unknown";
}

SessionOutputDrainPolicy default_session_output_drain_policy() {
    SessionOutputDrainPolicy policy;
    policy.idle_grace_ms = DEFAULT_EXIT_DRAIN_IDLE_GRACE_MS;
    policy.max_grace_ms = DEFAULT_EXIT_DRAIN_MAX_GRACE_MS;
    policy.terminate_quiet_ms = DEFAULT_EXIT_DRAIN_TERMINATE_QUIET_MS;
    return policy;
}

SessionOutputDrainResult::SessionOutputDrainResult()
    : completed(false), reason(SessionOutputDrainStopReason::None) {
}

SessionOutputDrainResult::SessionOutputDrainResult(
    bool completed_value,
    SessionOutputDrainStopReason reason_value
)
    : completed(completed_value), reason(reason_value) {
}

void wait_for_generation_change_locked(
    LiveSession* session,
    std::uint64_t baseline_generation,
    std::uint64_t deadline_ms,
    unsigned long max_wait_ms
) {
    while (!session->closing && session->output_.generation == baseline_generation) {
        if (platform::monotonic_deadline_expired(deadline_ms)) {
            return;
        }
        unsigned long remaining = platform::monotonic_deadline_remaining_ms(deadline_ms);
        if (max_wait_ms > 0UL) {
            remaining = std::min(remaining, max_wait_ms);
        }
        if (!session->cond_.timed_wait_ms(session->mutex_, remaining)) {
            return;
        }
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
        ensure_session_drain_started_locked(session);
        ++session->output_.generation;
        session->cond_.broadcast();
        return true;
    }
    return false;
}

void finish_session_output_locked(LiveSession* session, SessionOutputDrainStopReason reason) {
    const std::string flushed = session->process->flush_carry(&session->output_.decode_carry);
    if (!flushed.empty()) {
        session->output_.buffered_output += flushed;
        if (session->output_.exited) {
            note_session_drain_output_locked(session);
        }
    }
    session->output_.eof = true;
    mark_session_exit_locked(session);
    record_session_drain_stop_locked(session, reason);
    ++session->output_.generation;
    session->cond_.broadcast();
}

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
    join_daemon_thread(&thread);
}

std::string take_session_output_locked(LiveSession* session) {
    std::string output;
    output.swap(session->output_.buffered_output);
    return output;
}

SessionOutputDrainResult drain_exited_session_output_locked(
    LiveSession* session,
    std::string* output,
    const SessionOutputDrainPolicy& policy
) {
    ensure_session_drain_started_locked(session);

    SessionOutputDrainStopReason grace_stop_reason = SessionOutputDrainStopReason::None;
    for (;;) {
        if (!session->output_.buffered_output.empty()) {
            *output += take_session_output_locked(session);
            note_session_drain_output_locked(session);
        }
        if (session->output_.eof) {
            return make_session_drain_result_locked(session, true, eof_stop_reason_locked(session));
        }
        if (session->closing) {
            return make_session_drain_result_locked(
                session,
                true,
                SessionOutputDrainStopReason::StoreClosing
            );
        }

        std::uint64_t next_deadline_ms = 0ULL;
        grace_stop_reason = grace_deadline_stop_reason_locked(session, policy, &next_deadline_ms);
        if (grace_stop_reason != SessionOutputDrainStopReason::None) {
            break;
        }

        const std::uint64_t seen_generation = session->output_.generation;
        wait_for_generation_change_locked(session, seen_generation, next_deadline_ms, 0UL);
    }
    record_session_drain_stop_locked(session, grace_stop_reason);

    if (!session->output_.descendant_cleanup_attempted) {
        session->output_.descendant_cleanup_attempted = true;
        session->output_.descendant_cleanup_supported =
            terminate_descendants_after_exit_locked(session);
        session->output_.descendant_cleanup_started_at_ms = platform::monotonic_ms();
    }
    if (!session->output_.descendant_cleanup_supported) {
        return make_session_drain_result_locked(
            session,
            false,
            SessionOutputDrainStopReason::DescendantTerminateUnsupported
        );
    }

    for (;;) {
        if (!session->output_.buffered_output.empty()) {
            *output += take_session_output_locked(session);
        }
        if (session->output_.eof) {
            return make_session_drain_result_locked(session, true, eof_stop_reason_locked(session));
        }
        if (session->closing) {
            return make_session_drain_result_locked(
                session,
                true,
                SessionOutputDrainStopReason::StoreClosing
            );
        }

        const std::uint64_t terminate_deadline_ms = deadline_after_ms(
            session->output_.descendant_cleanup_started_at_ms,
            policy.terminate_quiet_ms
        );
        if (platform::monotonic_deadline_expired(terminate_deadline_ms)) {
            return make_session_drain_result_locked(
                session,
                false,
                SessionOutputDrainStopReason::DescendantTerminateTimeout
            );
        }

        const std::uint64_t seen_generation = session->output_.generation;
        wait_for_generation_change_locked(session, seen_generation, terminate_deadline_ms, 0UL);
    }
}
