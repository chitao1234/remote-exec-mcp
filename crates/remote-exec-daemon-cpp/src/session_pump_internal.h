#pragma once

#include <cstdint>
#include <string>

#include "live_session.h"

bool mark_session_exit_locked(LiveSession* session);
void finish_session_output_locked(LiveSession* session);
std::string take_session_output_locked(LiveSession* session, unsigned long max_output_tokens);
bool drain_exited_session_output_locked(LiveSession* session, std::string* output, unsigned long max_output_tokens);
void wait_for_generation_change_locked(LiveSession* session,
                                       std::uint64_t baseline_generation,
                                       std::uint64_t deadline_ms,
                                       unsigned long max_wait_ms);
