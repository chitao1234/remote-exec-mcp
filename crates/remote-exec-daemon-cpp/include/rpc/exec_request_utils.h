#pragma once

#include <stdexcept>
#include <string>

struct HttpRequest;
struct ExecRequestContext;

class ExecRequestFailure : public std::runtime_error {
public:
    ExecRequestFailure(int status, const std::string& code, const std::string& message);

    int status;
    std::string code;
    std::string message;
};

struct ExecPtySizeSpec {
    bool present;
    unsigned short rows;
    unsigned short cols;
};

struct ExecStartRequestSpec {
    std::string cmd;
    std::string workdir;
    std::string shell;
    bool login_requested;
    bool tty_requested;
    bool has_yield_time_ms;
    unsigned long yield_time_ms;
    unsigned long max_output_tokens;
};

struct ExecWriteRequestSpec {
    std::string daemon_session_id;
    std::string chars;
    bool has_yield_time_ms;
    unsigned long yield_time_ms;
    unsigned long max_output_tokens;
    ExecPtySizeSpec pty_size;
};

ExecStartRequestSpec prepare_exec_start_request(
    const ExecRequestContext& context,
    const HttpRequest& request
);
ExecWriteRequestSpec prepare_exec_write_request(const HttpRequest& request);
