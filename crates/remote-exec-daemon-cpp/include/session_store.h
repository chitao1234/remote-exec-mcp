#pragma once

#include <map>
#include <memory>
#include <stdexcept>
#include <string>

#include "basic_mutex.h"
#include "config.h"
#include "http_helpers.h"
#include "live_session.h"

static constexpr unsigned long DEFAULT_MAX_OUTPUT_TOKENS = 10000UL;

struct ExecStartRequestSpec;

class UnknownSessionError : public std::runtime_error {
public:
    explicit UnknownSessionError(const std::string& message) : std::runtime_error(message) {}
};

class SessionLimitError : public std::runtime_error {
public:
    explicit SessionLimitError(const std::string& message) : std::runtime_error(message) {}
};

class StdinClosedError : public std::runtime_error {
public:
    explicit StdinClosedError(const std::string& message) : std::runtime_error(message) {}
};

class SessionStore {
public:
    SessionStore();
    ~SessionStore();

    Json start_command(const std::string& target,
                       const ExecStartRequestSpec& request,
                       const YieldTimeConfig& yield_time,
                       unsigned long max_open_sessions);
    Json write_stdin(const std::string& daemon_session_id,
                     const std::string& chars,
                     bool has_yield_time_ms,
                     unsigned long yield_time_ms,
                     unsigned long max_output_tokens,
                     const YieldTimeConfig& yield_time,
                     bool has_pty_size,
                     unsigned short pty_rows,
                     unsigned short pty_cols);

private:
    bool reserve_pending_start(unsigned long max_open_sessions);
    bool prune_one_session_for_start(unsigned long max_open_sessions);

    BasicMutex mutex_;
    std::map<std::string, std::shared_ptr<LiveSession>> sessions_;
    unsigned long pending_starts_;
};
