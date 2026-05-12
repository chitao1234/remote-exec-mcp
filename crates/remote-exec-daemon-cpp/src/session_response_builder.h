#pragma once

#include <cstdint>
#include <string>

#include "http_helpers.h"

Json build_session_response(const char* daemon_session_id,
                            bool running,
                            std::uint64_t started_at_ms,
                            bool has_exit_code,
                            int exit_code,
                            const std::string& output,
                            unsigned long max_output_tokens,
                            const Json& warnings);
