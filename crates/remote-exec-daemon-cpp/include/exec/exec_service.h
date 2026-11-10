#pragma once

#include <atomic>
#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <string>
#include <vector>

#include "core/config.h"
#include "core/event_loop.h"
#include "exec/exec_types.h"
#include "http/http_helpers.h"
#include "platform/basic_mutex.h"
#include "runtime/route_context.h"

class BlockingPool;
struct ExecSession;
struct ExecWaiter;
struct ExecStartRequestSpec;
struct ExecWriteRequestSpec;

// Owns command sessions and advances them from one loop. Process spawn and
// stdin writes run on the blocking pool; output, exit detection, and waiter
// deadlines run on the session loop.
class ExecService {
public:
    ExecService(
        const ExecRequestContext& request_context,
        const std::string& target,
        const std::string& daemon_instance_id,
        const YieldTimeConfig& yield_time,
        unsigned long stdin_write_timeout_ms,
        unsigned long max_open_sessions,
        bool allow_exec,
        EventLoop* session_loop,
        BlockingPool* pool
    );
    ~ExecService();

    // Completion runs on the session loop.
    void start_async(
        const std::shared_ptr<HttpRequest>& request,
        const std::function<void(HttpResponse)>& done
    );
    void write_async(
        const std::shared_ptr<HttpRequest>& request,
        const std::function<void(HttpResponse)>& done
    );

    void tick();
    void cancel_all();
    bool supports_pty() const;

    ExecService(const ExecService&) = delete;
    ExecService& operator=(const ExecService&) = delete;

private:
    struct StartReservation;

    void start_prologue(
        const ExecStartRequestSpec& spec,
        const std::function<void(HttpResponse)>& done
    );
    void write_prologue(
        const ExecWriteRequestSpec& spec,
        const std::function<void(HttpResponse)>& done
    );
    std::shared_ptr<ExecSession> find_session(const std::string& id) const;
    std::shared_ptr<ExecSession> spawn_session(const ExecStartRequestSpec& spec);
    bool reserve_start_slot();
    bool prune_for_start();
    bool acquire_poll_slot(const std::shared_ptr<ExecSession>& session);
    void release_poll_slot(const std::shared_ptr<ExecSession>& session);
    void register_start(
        const std::shared_ptr<ExecSession>& session,
        unsigned long max_output_tokens,
        unsigned long yield_ms,
        const std::vector<ExecSessionWarning>& warnings,
        const std::function<void(HttpResponse)>& done
    );
    void register_write(
        const std::shared_ptr<ExecSession>& session,
        unsigned long max_output_tokens,
        unsigned long yield_ms,
        const std::function<void(HttpResponse)>& done
    );
    void drain_all();
    void drain_session(const std::shared_ptr<ExecSession>& session);
    void complete_waiters();
    void complete_waiter(const std::shared_ptr<ExecWaiter>& waiter);
    void attach_output(const std::shared_ptr<ExecSession>& session);
    void detach_output(const std::shared_ptr<ExecSession>& session);
    void remove_session(const std::shared_ptr<ExecSession>& session);
    void retire_session(const std::shared_ptr<ExecSession>& session);
    HttpResponse make_response(
        const ExecSessionResult& result,
        unsigned long max_output_tokens
    ) const;
    HttpResponse disabled_response() const;
    HttpResponse busy_response() const;

    ExecRequestContext request_;
    std::string target_;
    std::string daemon_instance_id_;
    YieldTimeConfig yield_time_;
    unsigned long stdin_write_timeout_ms_;
    unsigned long max_open_sessions_;
    bool allow_exec_;
    EventLoop* session_loop_;
    BlockingPool* pool_;

    mutable BasicMutex mutex_;
    std::map<std::string, std::shared_ptr<ExecSession>> sessions_;
    unsigned long pending_starts_;

    // Session-loop-only state.
    std::map<std::uint64_t, std::shared_ptr<ExecWaiter>> waiters_;
    std::uint64_t next_waiter_id_;
    std::atomic<bool> cancelled_;
};
