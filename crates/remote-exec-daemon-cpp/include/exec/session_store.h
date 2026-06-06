#pragma once

#include <cstdint>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

#include "core/config.h"
#include "exec/live_session.h"
#include "platform/basic_mutex.h"

static constexpr unsigned long DEFAULT_MAX_OUTPUT_TOKENS = 10000UL;

struct ExecStartRequestSpec;

struct ExecSessionWarning {
    ExecSessionWarning() {}
    ExecSessionWarning(const std::string& code_value, const std::string& message_value)
        : code(code_value), message(message_value) {}

    std::string code;
    std::string message;
};

struct ExecSessionResult {
    ExecSessionResult()
        : has_daemon_session_id(false), running(false), started_at_ms(0), has_exit_code(false), exit_code(0) {}

    std::string daemon_session_id;
    bool has_daemon_session_id;
    bool running;
    std::uint64_t started_at_ms;
    bool has_exit_code;
    int exit_code;
    std::string output;
    std::vector<ExecSessionWarning> warnings;
};

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

    // Owns daemon-local exec session IDs. start_command() installs a LiveSession
    // after process launch succeeds; write_stdin() may retire and remove a
    // completed session. Destruction retires all remaining sessions, terminates
    // their processes, and joins output pump threads outside the store mutex.
    ExecSessionResult start_command(const std::string& target,
                                    const ExecStartRequestSpec& request,
                                    const YieldTimeConfig& yield_time,
                                    unsigned long max_open_sessions);
    ExecSessionResult write_stdin(const std::string& daemon_session_id,
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
