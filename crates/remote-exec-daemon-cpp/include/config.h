#pragma once

#include <cstddef>
#include <string>

#include "filesystem_sandbox.h"

struct YieldTimeOperationConfig {
    unsigned long default_ms;
    unsigned long max_ms;
    unsigned long min_ms;
};

struct YieldTimeConfig {
    YieldTimeOperationConfig exec_command;
    YieldTimeOperationConfig write_stdin_poll;
    YieldTimeOperationConfig write_stdin_input;
};

struct DaemonConfig {
    std::string target;
    std::string listen_host;
    int listen_port;
    std::string default_workdir;
    std::string default_shell;
    bool allow_login_shell;
    std::string http_auth_bearer_token;
    std::size_t max_request_header_bytes;
    std::size_t max_request_body_bytes;
    unsigned long max_open_sessions;
    YieldTimeConfig yield_time;
    bool sandbox_configured = false;
    FilesystemSandbox sandbox;
};

YieldTimeConfig default_yield_time_config();
unsigned long resolve_yield_time_ms(
    const YieldTimeOperationConfig& config,
    bool has_requested_ms,
    unsigned long requested_ms
);

DaemonConfig load_config(const std::string& path);
