#pragma once

#include <map>
#include <memory>
#include <string>

#include "config.h"
#include "http_helpers.h"
#include "win32_scoped.h"

struct LiveSession {
    std::string id;
    UniqueHandle process_handle;
    UniqueHandle stdin_write;
    UniqueHandle stdout_read;
    DWORD started_at_ms;
    std::string output_carry;
};

class SessionStore {
public:
    SessionStore();
    ~SessionStore();

    Json start_command(
        const std::string& command,
        const std::string& workdir,
        const std::string& shell,
        bool has_yield_time_ms,
        unsigned long yield_time_ms,
        unsigned long max_output_tokens,
        const YieldTimeConfig& yield_time
    );
    Json write_stdin(
        const std::string& daemon_session_id,
        const std::string& chars,
        bool has_yield_time_ms,
        unsigned long yield_time_ms,
        unsigned long max_output_tokens,
        const YieldTimeConfig& yield_time
    );

private:
    std::map<std::string, std::shared_ptr<LiveSession> > sessions_;
};
