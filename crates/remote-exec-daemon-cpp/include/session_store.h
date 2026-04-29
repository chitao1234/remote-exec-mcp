#pragma once

#include <cstdint>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>

#include "config.h"
#include "http_helpers.h"

class ProcessSession;

class UnknownSessionError : public std::runtime_error {
public:
    explicit UnknownSessionError(const std::string& message) : std::runtime_error(message) {}
};

class SessionLimitError : public std::runtime_error {
public:
    explicit SessionLimitError(const std::string& message) : std::runtime_error(message) {}
};

struct LiveSession {
    LiveSession();
    ~LiveSession();

    std::string id;
    std::unique_ptr<ProcessSession> process;
    std::uint64_t started_at_ms;
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
        bool login,
        bool has_yield_time_ms,
        unsigned long yield_time_ms,
        unsigned long max_output_chars,
        const YieldTimeConfig& yield_time,
        unsigned long max_open_sessions
    );
    Json write_stdin(
        const std::string& daemon_session_id,
        const std::string& chars,
        bool has_yield_time_ms,
        unsigned long yield_time_ms,
        unsigned long max_output_chars,
        const YieldTimeConfig& yield_time
    );

private:
    std::map<std::string, std::shared_ptr<LiveSession> > sessions_;
};
