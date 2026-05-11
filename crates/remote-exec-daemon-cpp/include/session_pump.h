#pragma once

#include <memory>
#include <string>

#include "session_store.h"

bool mark_session_exit_locked(LiveSession* session);
void finish_session_output_locked(LiveSession* session);
void start_session_pump(const std::shared_ptr<LiveSession>& session);
void join_session_pump(LiveSession* session);
std::string take_session_output_locked(LiveSession* session, unsigned long max_output_tokens);
bool drain_exited_session_output_locked(
    LiveSession* session,
    std::string* output,
    unsigned long max_output_tokens
);
