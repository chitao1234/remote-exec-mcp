#include "exec/exec_service.h"

#include <algorithm>
#include <cstdint>
#include <exception>
#include <sstream>
#include <utility>
#include <vector>

#include "core/blocking_pool.h"
#include "core/logging.h"
#include "core/text_utils.h"
#include "exec/bounded_transcript.h"
#include "exec/exec_types.h"
#include "exec/process_session.h"
#include "platform/platform.h"
#include "rpc/exec_http_codec.h"
#include "rpc/exec_request_utils.h"
#include "rpc/server_route_common.h"

namespace {

const std::size_t EXEC_OUTPUT_LIMIT = 1024U * 1024U;
const unsigned long EXIT_DRAIN_IDLE_MS = 250UL;
const unsigned long EXIT_DRAIN_MAX_MS = 2000UL;
const unsigned long RECENT_PROTECTED_SESSION_COUNT = 8UL;
const unsigned long WARNING_THRESHOLD_HEADROOM = 4UL;
const unsigned long POLL_SLOT_WAIT_SLICE_MS = 1000UL;

std::atomic<unsigned long> g_next_session_id(1UL);
std::atomic<std::uint64_t> g_next_touch_order(1ULL);

std::string make_session_id() {
    std::ostringstream out;
    out << "sess_" << platform::monotonic_ms() << "_" << g_next_session_id.fetch_add(1UL);
    return out.str();
}

std::uint64_t make_touch_order() {
    return g_next_touch_order.fetch_add(1ULL);
}

unsigned long warning_threshold(unsigned long max_open_sessions) {
    return max_open_sessions > WARNING_THRESHOLD_HEADROOM
               ? max_open_sessions - WARNING_THRESHOLD_HEADROOM
               : 0UL;
}

bool crosses_warning_threshold(std::size_t open_sessions, unsigned long max_open_sessions) {
    const unsigned long threshold = warning_threshold(max_open_sessions);
    const std::size_t next_open_sessions = open_sessions + 1U;
    return open_sessions < threshold && next_open_sessions >= threshold;
}

std::size_t protected_recent_count(std::size_t open_sessions) {
    if (open_sessions == 0U) {
        return 0U;
    }
    return std::min<std::size_t>(RECENT_PROTECTED_SESSION_COUNT, open_sessions - 1U);
}

} // namespace

struct ExecSession {
    ExecSession()
        : started_at_ms(0U), last_touched(0U), output_fd(-1), output_registered(false),
          transcript(EXEC_OUTPUT_LIMIT), exited(false), exit_code(0), exited_at_ms(0U), eof(false),
          descendant_cleanup_requested(false), completed(false), retired(false), stdin_open(false),
          poll_active(false) {}

    std::string id;
    std::unique_ptr<ProcessSession> process;
    std::uint64_t started_at_ms;
    std::atomic<std::uint64_t> last_touched;
    int output_fd;
    bool output_registered;
    BoundedTranscript transcript;
    std::string decode_carry;
    std::atomic<bool> exited;
    int exit_code;
    std::uint64_t exited_at_ms;
    bool eof;
    bool descendant_cleanup_requested;
    std::atomic<bool> completed;
    std::atomic<bool> retired;
    bool stdin_open;
    std::atomic<bool> poll_active;
    BasicMutex poll_mutex;
    BasicCondVar poll_cond;
};

struct ExecWaiter {
    ExecWaiter()
        : deadline_ms(0U), max_output_tokens(0U), output(EXEC_OUTPUT_LIMIT), original_bytes(0U) {}

    std::shared_ptr<ExecSession> session;
    std::uint64_t deadline_ms;
    unsigned long max_output_tokens;
    BoundedTranscript output;
    std::uint64_t original_bytes;
    std::vector<ExecSessionWarning> warnings;
    std::function<void(HttpResponse)> done;
};

struct ExecService::StartReservation {
    explicit StartReservation(ExecService* service) : service_(service), active_(true) {}

    ~StartReservation() { release(); }

    void release_locked() {
        if (!active_) {
            return;
        }
        if (service_->pending_starts_ > 0UL) {
            --service_->pending_starts_;
        }
        active_ = false;
    }

    void release() {
        if (!active_) {
            return;
        }
        BasicLockGuard lock(service_->mutex_);
        release_locked();
    }

    ExecService* service_;
    bool active_;
};

ExecService::ExecService(
    const ExecRequestContext& request_context,
    const std::string& target,
    const std::string& daemon_instance_id,
    const YieldTimeConfig& yield_time,
    unsigned long stdin_write_timeout_ms,
    unsigned long max_open_sessions,
    bool allow_exec,
    EventLoop* session_loop,
    BlockingPool* pool
)
    : request_(request_context), target_(target), daemon_instance_id_(daemon_instance_id),
      yield_time_(yield_time), stdin_write_timeout_ms_(stdin_write_timeout_ms),
      max_open_sessions_(max_open_sessions), allow_exec_(allow_exec), session_loop_(session_loop),
      pool_(pool), pending_starts_(0UL), next_waiter_id_(1U), cancelled_(false) {
}

ExecService::~ExecService() {
    cancel_all();
}

bool ExecService::supports_pty() const {
    return process_session_supports_pty();
}

void ExecService::start_async(
    const std::shared_ptr<HttpRequest>& request,
    const std::function<void(HttpResponse)>& done
) {
    if (!allow_exec_) {
        done(disabled_response());
        return;
    }
    if (cancelled_.load()) {
        done(busy_response());
        return;
    }
    const bool posted = pool_->post([this, request, done]() {
        if (cancelled_.load()) {
            done(busy_response());
            return;
        }
        try {
            const ExecStartRequestSpec spec = prepare_exec_start_request(request_, *request);
            start_prologue(spec, done);
        } catch (...) {
            done(exec_exception_response(
                "exec/start",
                ExecRouteKind::Start,
                std::current_exception()
            ));
        }
    });
    if (!posted) {
        done(busy_response());
    }
}

void ExecService::write_async(
    const std::shared_ptr<HttpRequest>& request,
    const std::function<void(HttpResponse)>& done
) {
    if (!allow_exec_) {
        done(disabled_response());
        return;
    }
    if (cancelled_.load()) {
        done(busy_response());
        return;
    }
    const bool posted = pool_->post([this, request, done]() {
        if (cancelled_.load()) {
            done(busy_response());
            return;
        }
        try {
            const ExecWriteRequestSpec spec = prepare_exec_write_request(*request);
            write_prologue(spec, done);
        } catch (...) {
            done(exec_exception_response(
                "exec/write",
                ExecRouteKind::Write,
                std::current_exception()
            ));
        }
    });
    if (!posted) {
        done(busy_response());
    }
}

std::shared_ptr<ExecSession> ExecService::find_session(const std::string& id) const {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<ExecSession>>::const_iterator it = sessions_.find(id);
    if (it == sessions_.end()) {
        return std::shared_ptr<ExecSession>();
    }
    return it->second;
}

std::shared_ptr<ExecSession> ExecService::spawn_session(const ExecStartRequestSpec& spec) {
    std::shared_ptr<ExecSession> session(new ExecSession());
    session->id = make_session_id();
    session->process = ProcessSession::launch(
        spec.cmd,
        spec.workdir,
        spec.shell,
        spec.windows_posix_root,
        spec.login_requested,
        spec.tty_requested,
        stdin_write_timeout_ms_
    );
    session->started_at_ms = platform::monotonic_ms();
    session->output_fd = session->process->output_fd();
#ifdef _WIN32
    session->stdin_open = true;
#else
    session->stdin_open = spec.tty_requested;
#endif
    session->last_touched.store(make_touch_order());
    session->poll_active.store(true);
    return session;
}

bool ExecService::reserve_start_slot() {
    for (;;) {
        {
            BasicLockGuard lock(mutex_);
            if (sessions_.size() + pending_starts_ < max_open_sessions_) {
                ++pending_starts_;
                return true;
            }
        }
        if (!prune_for_start()) {
            return false;
        }
    }
}

bool ExecService::prune_for_start() {
    struct Candidate {
        std::string id;
        std::shared_ptr<ExecSession> session;
        std::uint64_t touch;
    };

    for (;;) {
        std::vector<Candidate> snapshot;
        {
            BasicLockGuard lock(mutex_);
            if (sessions_.size() + pending_starts_ < max_open_sessions_) {
                return true;
            }
            if (sessions_.empty()) {
                return false;
            }
            snapshot.reserve(sessions_.size());
            for (std::map<std::string, std::shared_ptr<ExecSession>>::const_iterator it =
                     sessions_.begin();
                 it != sessions_.end();
                 ++it) {
                Candidate candidate;
                candidate.id = it->first;
                candidate.session = it->second;
                candidate.touch = it->second->last_touched.load();
                snapshot.push_back(candidate);
            }
        }

        std::sort(
            snapshot.begin(),
            snapshot.end(),
            [](const Candidate& left, const Candidate& right) {
                if (left.touch != right.touch) {
                    return left.touch < right.touch;
                }
                return left.id < right.id;
            }
        );

        const std::size_t prunable = snapshot.size() - protected_recent_count(snapshot.size());
        if (prunable == 0U) {
            return false;
        }

        std::shared_ptr<ExecSession> victim;
        for (std::size_t i = 0; i < prunable; ++i) {
            if (snapshot[i].session->poll_active.load()) {
                continue;
            }
            if (snapshot[i].session->completed.load() || snapshot[i].session->exited.load()) {
                victim = snapshot[i].session;
                break;
            }
        }
        if (!victim) {
            for (std::size_t i = 0; i < prunable; ++i) {
                if (!snapshot[i].session->poll_active.load()) {
                    victim = snapshot[i].session;
                    break;
                }
            }
        }
        if (!victim) {
            return false;
        }

        bool removed = false;
        {
            BasicLockGuard lock(mutex_);
            std::map<std::string, std::shared_ptr<ExecSession>>::iterator it =
                sessions_.find(victim->id);
            if (it != sessions_.end() && it->second == victim) {
                sessions_.erase(it);
                removed = true;
            }
        }
        if (!removed) {
            continue;
        }

        retire_session(victim);
        log_message(
            LOG_WARN,
            "exec_service",
            LogMessageBuilder("pruned exec session")
                .quoted_field("daemon_session_id", victim->id)
                .str()
        );
        return true;
    }
}

void ExecService::start_prologue(
    const ExecStartRequestSpec& spec,
    const std::function<void(HttpResponse)>& done
) {
    if (!reserve_start_slot()) {
        throw SessionLimitError("too many open exec sessions");
    }
    StartReservation reservation(this);

    const unsigned long yield_ms =
        resolve_yield_time_ms(yield_time_.exec_command, spec.has_yield_time_ms, spec.yield_time_ms);
    std::shared_ptr<ExecSession> session = spawn_session(spec);
    const unsigned long max_output_tokens = spec.max_output_tokens;

    std::vector<ExecSessionWarning> warnings;
    {
        BasicLockGuard lock(mutex_);
        const std::size_t open_before = sessions_.size();
        reservation.release_locked();
        session->last_touched.store(make_touch_order());
        sessions_[session->id] = session;
        if (crosses_warning_threshold(open_before, max_open_sessions_)) {
            ExecSessionWarning warning(
                "exec_session_limit_approaching",
                "Target `" + target_ + "` now has " + std::to_string(open_before + 1U)
                    + " open exec sessions."
            );
            warnings.push_back(warning);
        }
    }

    LogMessageBuilder message("start_command");
    message.quoted_field("cmd_preview", preview_text(spec.cmd, LOG_PREVIEW_LIMIT))
        .quoted_field("workdir", spec.workdir)
        .quoted_field("shell", spec.shell)
        .bool_field("login", spec.login_requested)
        .bool_field("tty", spec.tty_requested)
        .quoted_field("daemon_session_id", session->id);
    log_message(LOG_INFO, "exec_service", message.str());

    session_loop_->post([this, session, max_output_tokens, yield_ms, warnings, done]() {
        register_start(session, max_output_tokens, yield_ms, warnings, done);
    });
}

void ExecService::write_prologue(
    const ExecWriteRequestSpec& spec,
    const std::function<void(HttpResponse)>& done
) {
    std::shared_ptr<ExecSession> session = find_session(spec.daemon_session_id);
    if (!session) {
        throw UnknownSessionError("Unknown daemon session");
    }
    session->last_touched.store(make_touch_order());
    if (!acquire_poll_slot(session)) {
        throw UnknownSessionError("Unknown daemon session");
    }

    bool slot_released = false;
    struct SlotGuard {
        SlotGuard(
            ExecService* service_value,
            const std::shared_ptr<ExecSession>& session_value,
            bool* released_value
        )
            : service(service_value), session(session_value), released(released_value) {}

        ~SlotGuard() {
            if (released != nullptr && !*released) {
                service->release_poll_slot(session);
            }
        }

        ExecService* service;
        std::shared_ptr<ExecSession> session;
        bool* released;
    } guard(this, session, &slot_released);

    if (spec.pty_size.present) {
        if (spec.pty_size.rows == 0U || spec.pty_size.cols == 0U) {
            throw ProcessPtyResizeUnsupportedError("PTY rows and cols must be greater than zero");
        }
    }
    if (!spec.chars.empty() && !session->stdin_open) {
        throw StdinClosedError(
            "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
        );
    }

    if (spec.pty_size.present) {
        session->process->resize_pty(spec.pty_size.rows, spec.pty_size.cols);
    }

    if (!spec.chars.empty()) {
        try {
            session->process->write_stdin(spec.chars);
        } catch (const ProcessStdinWriteTimeoutError& ex) {
            session->stdin_open = false;
            retire_session(session);
            throw StdinWriteTimeoutError(ex.what());
        } catch (const ProcessStdinClosedError& ex) {
            session->stdin_open = false;
            throw StdinClosedError(ex.what());
        }
    }

    LogMessageBuilder message("write_stdin");
    message.quoted_field("daemon_session_id", spec.daemon_session_id)
        .field("chars_len", spec.chars.size());
    log_message(LOG_INFO, "exec_service", message.str());

    const unsigned long yield_ms = resolve_yield_time_ms(
        spec.chars.empty() ? yield_time_.write_stdin_poll : yield_time_.write_stdin_input,
        spec.has_yield_time_ms,
        spec.yield_time_ms
    );
    const unsigned long max_output_tokens = spec.max_output_tokens;
    session_loop_->post([this, session, max_output_tokens, yield_ms, done]() {
        register_write(session, max_output_tokens, yield_ms, done);
    });
    slot_released = true;
}

bool ExecService::acquire_poll_slot(const std::shared_ptr<ExecSession>& session) {
    BasicLockGuard lock(session->poll_mutex);
    while (session->poll_active.load() && !session->retired.load() && !cancelled_.load()) {
        (void)session->poll_cond.timed_wait_ms(session->poll_mutex, POLL_SLOT_WAIT_SLICE_MS);
    }
    if (session->retired.load() || cancelled_.load()) {
        return false;
    }
    session->poll_active.store(true);
    return true;
}

void ExecService::release_poll_slot(const std::shared_ptr<ExecSession>& session) {
    BasicLockGuard lock(session->poll_mutex);
    session->poll_active.store(false);
    session->poll_cond.broadcast();
}

void ExecService::register_start(
    const std::shared_ptr<ExecSession>& session,
    unsigned long max_output_tokens,
    unsigned long yield_ms,
    const std::vector<ExecSessionWarning>& warnings,
    const std::function<void(HttpResponse)>& done
) {
    attach_output(session);
    std::shared_ptr<ExecWaiter> waiter(new ExecWaiter());
    waiter->session = session;
    waiter->deadline_ms = platform::monotonic_ms() + static_cast<std::uint64_t>(yield_ms);
    waiter->max_output_tokens = max_output_tokens;
    waiter->warnings = warnings;
    waiter->done = done;
    waiters_[next_waiter_id_++] = waiter;
}

void ExecService::register_write(
    const std::shared_ptr<ExecSession>& session,
    unsigned long max_output_tokens,
    unsigned long yield_ms,
    const std::function<void(HttpResponse)>& done
) {
    attach_output(session);
    std::shared_ptr<ExecWaiter> waiter(new ExecWaiter());
    waiter->session = session;
    waiter->deadline_ms = platform::monotonic_ms() + static_cast<std::uint64_t>(yield_ms);
    waiter->max_output_tokens = max_output_tokens;
    waiter->done = done;

    if (session->retired.load() || session->completed.load()) {
        complete_waiter(waiter);
        return;
    }
    waiters_[next_waiter_id_++] = waiter;
}

void ExecService::tick() {
    drain_all();

    std::vector<std::uint64_t> due;
    const std::uint64_t now = platform::monotonic_ms();
    for (std::map<std::uint64_t, std::shared_ptr<ExecWaiter>>::iterator it = waiters_.begin();
         it != waiters_.end();
         ++it) {
        const std::shared_ptr<ExecSession>& session = it->second->session;
        if (!session->transcript.empty()) {
            it->second->original_bytes += session->transcript.total_bytes();
            it->second->output.append(session->transcript.take());
        }
        if (session->completed.load() || session->retired.load()
            || now >= it->second->deadline_ms) {
            due.push_back(it->first);
        }
    }

    for (std::size_t i = 0; i < due.size(); ++i) {
        std::map<std::uint64_t, std::shared_ptr<ExecWaiter>>::iterator it = waiters_.find(due[i]);
        if (it == waiters_.end()) {
            continue;
        }
        std::shared_ptr<ExecWaiter> waiter = it->second;
        waiters_.erase(it);
        complete_waiter(waiter);
    }
}

void ExecService::drain_all() {
    std::vector<std::shared_ptr<ExecSession>> sessions;
    {
        BasicLockGuard lock(mutex_);
        sessions.reserve(sessions_.size());
        for (std::map<std::string, std::shared_ptr<ExecSession>>::const_iterator it =
                 sessions_.begin();
             it != sessions_.end();
             ++it) {
            sessions.push_back(it->second);
        }
    }
    for (std::size_t i = 0; i < sessions.size(); ++i) {
        drain_session(sessions[i]);
    }
}

void ExecService::drain_session(const std::shared_ptr<ExecSession>& session) {
    if (session->retired.load()) {
        return;
    }

    const std::uint64_t now = platform::monotonic_ms();
    if (!session->exited.load()) {
        int exit_code = 0;
        if (session->process->has_exited(&exit_code)) {
            session->exit_code = exit_code;
            session->exited_at_ms = now;
            session->exited.store(true);
        }
    }

    if (!session->eof) {
        bool eof = false;
        const std::string chunk =
            session->process->read_output(false, &eof, &session->decode_carry);
        if (!chunk.empty()) {
            session->transcript.append(chunk);
        }
        if (eof) {
            const std::string flushed = session->process->flush_carry(&session->decode_carry);
            if (!flushed.empty()) {
                session->transcript.append(flushed);
            }
            session->eof = true;
            detach_output(session);
        }
    }

    if (session->exited.load() && !session->eof) {
        if (!session->descendant_cleanup_requested
            && now - session->exited_at_ms >= static_cast<std::uint64_t>(EXIT_DRAIN_IDLE_MS)) {
            session->descendant_cleanup_requested = true;
            std::shared_ptr<ExecSession> captured = session;
            if (pool_ != nullptr) {
                (void)pool_->post([captured]() {
                    if (captured->process) {
                        captured->process->terminate_descendants();
                    }
                });
            }
        }
        if (now - session->exited_at_ms >= static_cast<std::uint64_t>(EXIT_DRAIN_MAX_MS)) {
            session->completed.store(true);
            detach_output(session);
        }
    }

    if (session->exited.load() && session->eof) {
        session->completed.store(true);
    }
}

void ExecService::complete_waiter(const std::shared_ptr<ExecWaiter>& waiter) {
    const std::shared_ptr<ExecSession>& session = waiter->session;
    if (!session->transcript.empty()) {
        waiter->original_bytes += session->transcript.total_bytes();
        waiter->output.append(session->transcript.take());
    }

    const bool completed = session->completed.load() || session->retired.load();
    ExecSessionResult result;
    result.started_at_ms = session->started_at_ms;
    result.running = !completed;
    result.has_daemon_session_id = !completed;
    if (!completed) {
        result.daemon_session_id = session->id;
    }
    result.has_exit_code = session->exited.load();
    result.exit_code = session->exit_code;
    result.output = waiter->output.take();
    result.original_output_bytes = waiter->original_bytes;
    result.warnings = waiter->warnings;

    if (completed) {
        remove_session(session);
    } else {
        release_poll_slot(session);
    }

    if (waiter->done) {
        waiter->done(make_response(result, waiter->max_output_tokens));
    }
}

void ExecService::attach_output(const std::shared_ptr<ExecSession>& session) {
    if (session_loop_ == nullptr || session->output_registered || session->output_fd < 0) {
        return;
    }
    std::weak_ptr<ExecSession> weak = session;
    if (session_loop_->add(session->output_fd, IO_READ, [this, weak](int) {
            std::shared_ptr<ExecSession> session = weak.lock();
            if (session) {
                drain_session(session);
            }
        })) {
        session->output_registered = true;
    }
}

void ExecService::detach_output(const std::shared_ptr<ExecSession>& session) {
    if (!session->output_registered || session_loop_ == nullptr) {
        return;
    }
    session_loop_->remove(session->output_fd);
    session->output_registered = false;
}

void ExecService::remove_session(const std::shared_ptr<ExecSession>& session) {
    bool removed = false;
    {
        BasicLockGuard lock(mutex_);
        std::map<std::string, std::shared_ptr<ExecSession>>::iterator it =
            sessions_.find(session->id);
        if (it != sessions_.end() && it->second == session) {
            sessions_.erase(it);
            removed = true;
        }
    }
    if (!removed) {
        return;
    }
    detach_output(session);
    session->retired.store(true);
    release_poll_slot(session);

    std::shared_ptr<ExecSession> captured = session;
    if (pool_ == nullptr || !pool_->post([captured]() {
            if (captured->process) {
                captured->process->terminate();
            }
        })) {
        if (session->process) {
            session->process->terminate();
        }
    }
}

void ExecService::retire_session(const std::shared_ptr<ExecSession>& session) {
    session->retired.store(true);
    release_poll_slot(session);
    if (session_loop_ != nullptr) {
        std::shared_ptr<ExecSession> captured = session;
        session_loop_->post([this, captured]() { detach_output(captured); });
    }

    std::shared_ptr<ExecSession> captured = session;
    if (pool_ == nullptr || !pool_->post([captured]() {
            if (captured->process) {
                captured->process->terminate();
            }
        })) {
        if (session->process) {
            session->process->terminate();
        }
    }
}

void ExecService::cancel_all() {
    cancelled_.store(true);
    std::vector<std::shared_ptr<ExecSession>> sessions;
    {
        BasicLockGuard lock(mutex_);
        sessions.reserve(sessions_.size());
        for (std::map<std::string, std::shared_ptr<ExecSession>>::const_iterator it =
                 sessions_.begin();
             it != sessions_.end();
             ++it) {
            sessions.push_back(it->second);
        }
        sessions_.clear();
        pending_starts_ = 0UL;
    }
    waiters_.clear();

    for (std::size_t i = 0; i < sessions.size(); ++i) {
        sessions[i]->retired.store(true);
        sessions[i]->completed.store(true);
        release_poll_slot(sessions[i]);
        detach_output(sessions[i]);
        if (sessions[i]->process) {
            sessions[i]->process->terminate();
        }
    }
}

HttpResponse ExecService::make_response(
    const ExecSessionResult& result,
    unsigned long max_output_tokens
) const {
    HttpResponse response;
    Json body = exec_session_result_json(result, max_output_tokens);
    body["daemon_instance_id"] = daemon_instance_id_;
    write_json(response, body);
    return response;
}

HttpResponse ExecService::disabled_response() const {
    return make_rpc_error_response(
        400,
        "exec_disabled",
        "command execution is disabled by daemon config"
    );
}

HttpResponse ExecService::busy_response() const {
    return make_rpc_error_response(503, "busy", "exec service is busy");
}
