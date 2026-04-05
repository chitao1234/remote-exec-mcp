#pragma once

#include <map>
#include <memory>
#include <string>

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
        unsigned long yield_time_ms,
        unsigned long max_output_chars
    );
    Json write_stdin(
        const std::string& daemon_session_id,
        const std::string& chars,
        unsigned long yield_time_ms,
        unsigned long max_output_chars
    );

private:
    std::map<std::string, std::shared_ptr<LiveSession> > sessions_;
};
