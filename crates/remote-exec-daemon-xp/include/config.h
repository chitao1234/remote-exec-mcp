#pragma once

#include <string>

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
    YieldTimeConfig yield_time;
};

YieldTimeConfig default_yield_time_config();
unsigned long resolve_yield_time_ms(
    const YieldTimeOperationConfig& config,
    bool has_requested_ms,
    unsigned long requested_ms
);

DaemonConfig load_config(const std::string& path);
