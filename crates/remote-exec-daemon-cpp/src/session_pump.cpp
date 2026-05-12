#include "session_pump.h"

#include <algorithm>
#include <stdexcept>
#include <string>

#include "logging.h"
#include "platform.h"
#include "process_session.h"
#ifdef _WIN32
#include "win32_thread.h"
#endif

namespace {

const unsigned long EXIT_DRAIN_INITIAL_WAIT_MS = 125UL;
const unsigned long EXIT_DRAIN_QUIET_MS = 25UL;

void append_session_output_locked(LiveSession* session, const std::string& chunk) {
    if (!chunk.empty()) {
        session->output_.buffered_output += chunk;
        ++session->output_.generation;
        session->cond_.broadcast();
    }
}

bool terminate_descendants_after_exit_locked(LiveSession* session) {
    if (session->process.get() != NULL) {
        return session->process->terminate_descendants();
    }
    return false;
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
        } catch (const std::exception& ex) {
            log_message(LOG_WARN, "session", std::string("session output pump failed: ") + ex.what());
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
    std::unique_ptr<SessionPumpContext> context(static_cast<SessionPumpContext*>(raw_context));
    pump_session_output(context->session);
    return 0;
}
#endif

} // namespace

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

#ifdef _WIN32
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

std::string take_session_output_locked(LiveSession* session, unsigned long max_output_tokens) {
    (void)max_output_tokens;
    std::string output = session->output_.buffered_output;
    session->output_.buffered_output.clear();
    return output;
}

bool drain_exited_session_output_locked(LiveSession* session, std::string* output, unsigned long max_output_tokens) {
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
