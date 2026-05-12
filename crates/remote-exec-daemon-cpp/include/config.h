#pragma once

#include <cstddef>
#include <string>

#include "filesystem_sandbox.h"
#include "transfer_ops.h"

static const unsigned long DEFAULT_YIELD_TIME_EXEC_COMMAND_DEFAULT_MS = 10000UL;
static const unsigned long DEFAULT_YIELD_TIME_EXEC_COMMAND_MAX_MS = 30000UL;
static const unsigned long DEFAULT_YIELD_TIME_EXEC_COMMAND_MIN_MS = 250UL;
static const unsigned long DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_DEFAULT_MS = 5000UL;
static const unsigned long DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MAX_MS = 300000UL;
static const unsigned long DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MIN_MS = 5000UL;
static const unsigned long DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_DEFAULT_MS = 250UL;
static const unsigned long DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MAX_MS = 30000UL;
static const unsigned long DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MIN_MS = 250UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS = 256UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS = 64UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS = 64UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_UDP_BINDS = 64UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS = 1024UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES = 8UL * 1024UL * 1024UL;
static const unsigned long DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS = 30000UL;
static const unsigned long DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS = 10000UL;
static const unsigned long DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS = 30000UL;

struct YieldTimeOperationConfig {
    YieldTimeOperationConfig() : default_ms(0UL), max_ms(0UL), min_ms(0UL) {}

    YieldTimeOperationConfig(unsigned long default_ms_value, unsigned long max_ms_value, unsigned long min_ms_value)
        : default_ms(default_ms_value), max_ms(max_ms_value), min_ms(min_ms_value) {}

    unsigned long default_ms;
    unsigned long max_ms;
    unsigned long min_ms;
};

struct YieldTimeConfig {
    YieldTimeConfig()
        : exec_command(DEFAULT_YIELD_TIME_EXEC_COMMAND_DEFAULT_MS,
                       DEFAULT_YIELD_TIME_EXEC_COMMAND_MAX_MS,
                       DEFAULT_YIELD_TIME_EXEC_COMMAND_MIN_MS),
          write_stdin_poll(DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_DEFAULT_MS,
                           DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MAX_MS,
                           DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MIN_MS),
          write_stdin_input(DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_DEFAULT_MS,
                            DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MAX_MS,
                            DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MIN_MS) {}

    YieldTimeOperationConfig exec_command;
    YieldTimeOperationConfig write_stdin_poll;
    YieldTimeOperationConfig write_stdin_input;
};

struct PortForwardLimitConfig {
    PortForwardLimitConfig()
        : max_worker_threads(DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS),
          max_retained_sessions(DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS),
          max_retained_listeners(DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS),
          max_udp_binds(DEFAULT_PORT_FORWARD_MAX_UDP_BINDS),
          max_active_tcp_streams(DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS),
          max_tunnel_queued_bytes(DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES),
          tunnel_io_timeout_ms(DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS),
          connect_timeout_ms(DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS) {}

    unsigned long max_worker_threads;
    unsigned long max_retained_sessions;
    unsigned long max_retained_listeners;
    unsigned long max_udp_binds;
    unsigned long max_active_tcp_streams;
    unsigned long max_tunnel_queued_bytes;
    unsigned long tunnel_io_timeout_ms;
    unsigned long connect_timeout_ms;
};

struct DaemonConfig {
    std::string target;
    std::string listen_host;
    int listen_port;
    std::string test_bound_addr_file;
    std::string default_workdir;
    std::string default_shell;
    bool allow_login_shell;
    std::string http_auth_bearer_token;
    std::size_t max_request_header_bytes;
    std::size_t max_request_body_bytes;
    unsigned long http_connection_idle_timeout_ms = DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS;
    TransferLimitConfig transfer_limits = default_transfer_limit_config();
    unsigned long max_open_sessions;
    PortForwardLimitConfig port_forward_limits;
    YieldTimeConfig yield_time;
    bool sandbox_configured = false;
    FilesystemSandbox sandbox;
};

unsigned long
resolve_yield_time_ms(const YieldTimeOperationConfig& config, bool has_requested_ms, unsigned long requested_ms);

DaemonConfig load_config(const std::string& path);
