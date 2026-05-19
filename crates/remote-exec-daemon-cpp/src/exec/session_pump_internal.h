#pragma once

#include <cstdint>
#include <string>

#include "live_session.h"

struct SessionOutputDrainPolicy {
    // After parent exit, drain descendant-held stdout/stderr until idle or max
    // grace, then give child cleanup a short quiet window.
    unsigned long idle_grace_ms;
    unsigned long max_grace_ms;
    unsigned long terminate_quiet_ms;
};

SessionOutputDrainPolicy default_session_output_drain_policy();
bool mark_session_exit_locked(LiveSession* session);
void finish_session_output_locked(LiveSession* session);
std::string take_session_output_locked(LiveSession* session, unsigned long max_output_tokens);
bool drain_exited_session_output_locked(LiveSession* session,
                                        std::string* output,
                                        unsigned long max_output_tokens,
                                        const SessionOutputDrainPolicy& policy);
void wait_for_generation_change_locked(LiveSession* session,
                                       std::uint64_t baseline_generation,
                                       std::uint64_t deadline_ms,
                                       unsigned long max_wait_ms);
