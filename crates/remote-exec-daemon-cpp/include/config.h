#pragma once

#include <cstddef>
#include <string>

#include "filesystem_sandbox.h"
#include "transfer_ops.h"

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

struct PortForwardLimitConfig {
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
    std::string default_workdir;
    std::string default_shell;
    bool allow_login_shell;
    std::string http_auth_bearer_token;
    std::size_t max_request_header_bytes;
    std::size_t max_request_body_bytes;
    unsigned long http_connection_idle_timeout_ms;
    TransferLimitConfig transfer_limits = default_transfer_limit_config();
    unsigned long max_open_sessions;
    PortForwardLimitConfig port_forward_limits;
    YieldTimeConfig yield_time;
    bool sandbox_configured = false;
    FilesystemSandbox sandbox;
};

static const unsigned long DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS = 256UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS = 64UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS = 64UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_UDP_BINDS = 64UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS = 1024UL;
static const unsigned long DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES = 8UL * 1024UL * 1024UL;
static const unsigned long DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS = 30000UL;
static const unsigned long DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS = 10000UL;
static const unsigned long DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS = 30000UL;

YieldTimeConfig default_yield_time_config();
PortForwardLimitConfig default_port_forward_limit_config();
unsigned long resolve_yield_time_ms(
    const YieldTimeOperationConfig& config,
    bool has_requested_ms,
    unsigned long requested_ms
);

DaemonConfig load_config(const std::string& path);
