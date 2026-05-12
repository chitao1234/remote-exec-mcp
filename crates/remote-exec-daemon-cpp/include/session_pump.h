#pragma once

#include <memory>

#include "live_session.h"

void start_session_pump(const std::shared_ptr<LiveSession>& session);
void join_session_pump(LiveSession* session);
