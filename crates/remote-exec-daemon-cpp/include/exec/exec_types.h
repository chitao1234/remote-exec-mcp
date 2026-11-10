#pragma once

#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>

static constexpr unsigned long DEFAULT_MAX_OUTPUT_TOKENS = 10000UL;

struct ExecSessionWarning {
    ExecSessionWarning() {}
    ExecSessionWarning(const std::string& code_value, const std::string& message_value)
        : code(code_value), message(message_value) {}

    std::string code;
    std::string message;
};

struct ExecSessionResult {
    ExecSessionResult()
        : has_daemon_session_id(false), running(false), started_at_ms(0), has_exit_code(false),
          exit_code(0), original_output_bytes(0) {}

    std::string daemon_session_id;
    bool has_daemon_session_id;
    bool running;
    std::uint64_t started_at_ms;
    bool has_exit_code;
    int exit_code;
    std::string output;
    std::uint64_t original_output_bytes;
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

class StdinWriteTimeoutError : public std::runtime_error {
public:
    explicit StdinWriteTimeoutError(const std::string& message) : std::runtime_error(message) {}
};
