#pragma once

#include <cstdint>
#include <string>

#include "exec/live_session.h"

struct SessionOutputDrainPolicy {
    // After parent exit, drain descendant-held stdout/stderr until either:
    // - no output arrives for idle_grace_ms after the latest drained chunk, or
    // - max_grace_ms elapses after parent exit is observed.
    //
    // Once a grace boundary is reached, ask the process backend to terminate
    // descendants once and wait up to terminate_quiet_ms for EOF.
    unsigned long idle_grace_ms;
    unsigned long max_grace_ms;
    unsigned long terminate_quiet_ms;
};

struct SessionOutputDrainResult {
    SessionOutputDrainResult();
    SessionOutputDrainResult(bool completed_value, SessionOutputDrainStopReason reason_value);

    bool completed;
    SessionOutputDrainStopReason reason;
};

SessionOutputDrainPolicy default_session_output_drain_policy();
bool mark_session_exit_locked(LiveSession* session);
void finish_session_output_locked(LiveSession* session, SessionOutputDrainStopReason reason);
std::string take_session_output_locked(LiveSession* session, unsigned long max_output_tokens);
SessionOutputDrainResult drain_exited_session_output_locked(LiveSession* session,
                                                            std::string* output,
                                                            unsigned long max_output_tokens,
                                                            const SessionOutputDrainPolicy& policy);
void wait_for_generation_change_locked(LiveSession* session,
                                       std::uint64_t baseline_generation,
                                       std::uint64_t deadline_ms,
                                       unsigned long max_wait_ms);
