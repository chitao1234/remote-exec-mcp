#include <cerrno>
#include <climits>
#include <cstdlib>
#include <fstream>
#include <map>
#include <limits>
#include <sstream>
#include <stdexcept>

#include "config.h"
#include "text_utils.h"

typedef std::map<std::string, std::string> ConfigValues;

static std::string unquote(const std::string& raw) {
    if (raw.size() >= 2 && raw.front() == '"' && raw.back() == '"') {
        return raw.substr(1, raw.size() - 2);
    }
    return raw;
}

static unsigned long parse_unsigned_long(
    const std::string& raw,
    const std::string& key
) {
    if (raw.empty()) {
        throw std::runtime_error("missing numeric value for " + key);
    }

    errno = 0;
    char* end = NULL;
    const unsigned long value = std::strtoul(raw.c_str(), &end, 10);
    if (errno == ERANGE || end == raw.c_str() || (end != NULL && *end != '\0')) {
        throw std::runtime_error("invalid numeric value for " + key + ": " + raw);
    }
    return value;
}

static unsigned long read_optional_unsigned_long(
    const ConfigValues& values,
    const std::string& key,
    unsigned long fallback
) {
    const ConfigValues::const_iterator it = values.find(key);
    if (it == values.end()) {
        return fallback;
    }
    return parse_unsigned_long(it->second, key);
}

static std::uint64_t parse_uint64(
    const std::string& raw,
    const std::string& key
) {
    if (raw.empty()) {
        throw std::runtime_error("missing numeric value for " + key);
    }

    errno = 0;
    char* end = NULL;
    const unsigned long long value = std::strtoull(raw.c_str(), &end, 10);
    if (errno == ERANGE || end == raw.c_str() || (end != NULL && *end != '\0')) {
        throw std::runtime_error("invalid numeric value for " + key + ": " + raw);
    }
    return static_cast<std::uint64_t>(value);
}

static std::uint64_t read_optional_uint64(
    const ConfigValues& values,
    const std::string& key,
    std::uint64_t fallback
) {
    const ConfigValues::const_iterator it = values.find(key);
    if (it == values.end()) {
        return fallback;
    }
    return parse_uint64(it->second, key);
}

static std::string read_required_string(
    const ConfigValues& values,
    const std::string& key
) {
    const ConfigValues::const_iterator it = values.find(key);
    if (it == values.end()) {
        throw std::runtime_error("missing required config key: " + key);
    }
    return it->second;
}

static std::string read_optional_string(
    const ConfigValues& values,
    const std::string& key,
    const std::string& fallback
) {
    const ConfigValues::const_iterator it = values.find(key);
    return it == values.end() ? fallback : it->second;
}

static bool read_optional_bool(
    const ConfigValues& values,
    const std::string& key,
    bool fallback
) {
    const ConfigValues::const_iterator it = values.find(key);
    if (it == values.end()) {
        return fallback;
    }
    if (it->second == "true" || it->second == "1" || it->second == "yes") {
        return true;
    }
    if (it->second == "false" || it->second == "0" || it->second == "no") {
        return false;
    }
    throw std::runtime_error("invalid boolean value for " + key + ": " + it->second);
}

static std::size_t read_optional_size_t(
    const ConfigValues& values,
    const std::string& key,
    std::size_t fallback
) {
    const ConfigValues::const_iterator it = values.find(key);
    if (it == values.end()) {
        return fallback;
    }
    const unsigned long value = parse_unsigned_long(it->second, key);
    return static_cast<std::size_t>(value);
}

static bool contains_ascii_whitespace(const std::string& value) {
    return value.find_first_of(" \t\r\n") != std::string::npos;
}

static std::vector<std::string> split_semicolon_list(const std::string& raw) {
    std::vector<std::string> values;
    std::size_t start = 0;
    while (start <= raw.size()) {
        const std::size_t end = raw.find(';', start);
        const std::string part = trim_ascii(
            raw.substr(start, end == std::string::npos ? std::string::npos : end - start)
        );
        if (!part.empty()) {
            values.push_back(part);
        }
        if (end == std::string::npos) {
            break;
        }
        start = end + 1;
    }
    return values;
}

static std::vector<std::string> read_optional_path_list(
    const ConfigValues& values,
    const std::string& key
) {
    const ConfigValues::const_iterator it = values.find(key);
    if (it == values.end()) {
        return std::vector<std::string>();
    }
    return split_semicolon_list(it->second);
}

static bool has_key_with_prefix(
    const ConfigValues& values,
    const std::string& prefix
) {
    for (ConfigValues::const_iterator it = values.begin();
         it != values.end();
         ++it) {
        if (it->first.rfind(prefix, 0) == 0) {
            return true;
        }
    }
    return false;
}

static void validate_yield_time_operation(
    const YieldTimeOperationConfig& config,
    const std::string& key_prefix
) {
    if (config.min_ms > config.max_ms) {
        throw std::runtime_error(
            key_prefix + "_min_ms must be less than or equal to " + key_prefix + "_max_ms"
        );
    }
    if (config.default_ms < config.min_ms || config.default_ms > config.max_ms) {
        throw std::runtime_error(
            key_prefix + "_default_ms must be between " + key_prefix + "_min_ms and " +
            key_prefix + "_max_ms"
        );
    }
}

static YieldTimeOperationConfig read_yield_time_operation(
    const ConfigValues& values,
    const std::string& key_prefix,
    const YieldTimeOperationConfig& defaults
) {
    YieldTimeOperationConfig config;
    config.default_ms = read_optional_unsigned_long(
        values,
        key_prefix + "_default_ms",
        defaults.default_ms
    );
    config.max_ms = read_optional_unsigned_long(
        values,
        key_prefix + "_max_ms",
        defaults.max_ms
    );
    config.min_ms = read_optional_unsigned_long(
        values,
        key_prefix + "_min_ms",
        defaults.min_ms
    );
    validate_yield_time_operation(config, key_prefix);
    return config;
}

YieldTimeConfig default_yield_time_config() {
    YieldTimeConfig config;
    config.exec_command = YieldTimeOperationConfig{10000UL, 30000UL, 250UL};
    config.write_stdin_poll = YieldTimeOperationConfig{5000UL, 300000UL, 5000UL};
    config.write_stdin_input = YieldTimeOperationConfig{250UL, 30000UL, 250UL};
    return config;
}

PortForwardLimitConfig default_port_forward_limit_config() {
    PortForwardLimitConfig config;
    config.max_worker_threads = DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS;
    config.max_retained_sessions = DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS;
    config.max_retained_listeners = DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS;
    config.max_udp_binds = DEFAULT_PORT_FORWARD_MAX_UDP_BINDS;
    config.max_active_tcp_streams = DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS;
    config.max_tunnel_queued_bytes = DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES;
    config.tunnel_io_timeout_ms = DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS;
    config.connect_timeout_ms = DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS;
    return config;
}

static void validate_port_forward_limit(
    unsigned long value,
    const std::string& key
) {
    if (value == 0UL) {
        throw std::runtime_error(key + " must be greater than zero");
    }
}

static ConfigValues read_config_values(const std::string& path) {
    std::ifstream input(path.c_str());
    if (!input) {
        throw std::runtime_error("unable to open config file: " + path);
    }

    ConfigValues values;
    std::string line;
    while (std::getline(input, line)) {
        line = trim_ascii(line);
        if (line.empty() || line[0] == '#' || line[0] == ';') {
            continue;
        }

        const std::size_t equals = line.find('=');
        if (equals == std::string::npos) {
            throw std::runtime_error("invalid config line: " + line);
        }

        const std::string key = trim_ascii(line.substr(0, equals));
        const std::string value = unquote(trim_ascii(line.substr(equals + 1)));
        values[key] = value;
    }
    return values;
}

static int read_listen_port(const ConfigValues& values) {
    const unsigned long listen_port =
        parse_unsigned_long(read_required_string(values, "listen_port"), "listen_port");
    if (listen_port == 0 || listen_port > 65535UL) {
        throw std::runtime_error("listen_port must be between 1 and 65535");
    }
    return static_cast<int>(listen_port);
}

static std::string read_http_auth_bearer_token(const ConfigValues& values) {
    const ConfigValues::const_iterator it = values.find("http_auth_bearer_token");
    if (it == values.end()) {
        return "";
    }
    if (it->second.empty()) {
        throw std::runtime_error("http_auth_bearer_token must not be empty");
    }
    if (contains_ascii_whitespace(it->second)) {
        throw std::runtime_error("http_auth_bearer_token must not contain whitespace");
    }
    return it->second;
}

static PortForwardLimitConfig read_port_forward_limits(const ConfigValues& values) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_worker_threads = read_optional_unsigned_long(
        values,
        "port_forward_max_worker_threads",
        limits.max_worker_threads
    );
    limits.max_retained_sessions = read_optional_unsigned_long(
        values,
        "port_forward_max_retained_sessions",
        limits.max_retained_sessions
    );
    limits.max_retained_listeners = read_optional_unsigned_long(
        values,
        "port_forward_max_retained_listeners",
        limits.max_retained_listeners
    );
    limits.max_udp_binds = read_optional_unsigned_long(
        values,
        "port_forward_max_udp_binds",
        limits.max_udp_binds
    );
    limits.max_active_tcp_streams = read_optional_unsigned_long(
        values,
        "port_forward_max_active_tcp_streams",
        limits.max_active_tcp_streams
    );
    limits.max_tunnel_queued_bytes = read_optional_unsigned_long(
        values,
        "port_forward_max_tunnel_queued_bytes",
        limits.max_tunnel_queued_bytes
    );
    limits.tunnel_io_timeout_ms = read_optional_unsigned_long(
        values,
        "port_forward_tunnel_io_timeout_ms",
        limits.tunnel_io_timeout_ms
    );
    limits.connect_timeout_ms = read_optional_unsigned_long(
        values,
        "port_forward_connect_timeout_ms",
        limits.connect_timeout_ms
    );
    return limits;
}

static void validate_port_forward_limits(const PortForwardLimitConfig& limits) {
    validate_port_forward_limit(
        limits.max_worker_threads,
        "port_forward_max_worker_threads"
    );
    validate_port_forward_limit(
        limits.max_retained_sessions,
        "port_forward_max_retained_sessions"
    );
    validate_port_forward_limit(
        limits.max_retained_listeners,
        "port_forward_max_retained_listeners"
    );
    validate_port_forward_limit(limits.max_udp_binds, "port_forward_max_udp_binds");
    validate_port_forward_limit(
        limits.max_active_tcp_streams,
        "port_forward_max_active_tcp_streams"
    );
    validate_port_forward_limit(
        limits.max_tunnel_queued_bytes,
        "port_forward_max_tunnel_queued_bytes"
    );
    validate_port_forward_limit(
        limits.tunnel_io_timeout_ms,
        "port_forward_tunnel_io_timeout_ms"
    );
    validate_port_forward_limit(
        limits.connect_timeout_ms,
        "port_forward_connect_timeout_ms"
    );
}

static YieldTimeConfig read_yield_time_config(const ConfigValues& values) {
    YieldTimeConfig config = default_yield_time_config();
    config.exec_command = read_yield_time_operation(
        values,
        "yield_time_exec_command",
        config.exec_command
    );
    config.write_stdin_poll = read_yield_time_operation(
        values,
        "yield_time_write_stdin_poll",
        config.write_stdin_poll
    );
    config.write_stdin_input = read_yield_time_operation(
        values,
        "yield_time_write_stdin_input",
        config.write_stdin_input
    );
    return config;
}

static TransferLimitConfig read_transfer_limits(const ConfigValues& values) {
    TransferLimitConfig limits = default_transfer_limit_config();
    limits.max_archive_bytes = read_optional_uint64(
        values,
        "transfer_max_archive_bytes",
        limits.max_archive_bytes
    );
    limits.max_entry_bytes = read_optional_uint64(
        values,
        "transfer_max_entry_bytes",
        limits.max_entry_bytes
    );
    return limits;
}

static void validate_transfer_limits(const TransferLimitConfig& limits) {
    if (limits.max_archive_bytes == 0ULL) {
        throw std::runtime_error("transfer_max_archive_bytes must be greater than zero");
    }
    if (limits.max_entry_bytes == 0ULL) {
        throw std::runtime_error("transfer_max_entry_bytes must be greater than zero");
    }
    if (limits.max_entry_bytes > limits.max_archive_bytes) {
        throw std::runtime_error(
            "transfer_max_entry_bytes must be less than or equal to transfer_max_archive_bytes"
        );
    }
}

static FilesystemSandbox read_sandbox(const ConfigValues& values) {
    FilesystemSandbox sandbox;
    sandbox.exec_cwd.allow = read_optional_path_list(values, "sandbox_exec_cwd_allow");
    sandbox.exec_cwd.deny = read_optional_path_list(values, "sandbox_exec_cwd_deny");
    sandbox.read.allow = read_optional_path_list(values, "sandbox_read_allow");
    sandbox.read.deny = read_optional_path_list(values, "sandbox_read_deny");
    sandbox.write.allow = read_optional_path_list(values, "sandbox_write_allow");
    sandbox.write.deny = read_optional_path_list(values, "sandbox_write_deny");
    return sandbox;
}

static void validate_daemon_config(const DaemonConfig& config) {
    if (config.max_request_header_bytes == 0) {
        throw std::runtime_error("max_request_header_bytes must be greater than zero");
    }
    if (config.max_request_body_bytes == 0) {
        throw std::runtime_error("max_request_body_bytes must be greater than zero");
    }
    if (config.max_open_sessions == 0) {
        throw std::runtime_error("max_open_sessions must be greater than zero");
    }
    validate_port_forward_limits(config.port_forward_limits);
    validate_transfer_limits(config.transfer_limits);
}

unsigned long resolve_yield_time_ms(
    const YieldTimeOperationConfig& config,
    bool has_requested_ms,
    unsigned long requested_ms
) {
    unsigned long value = has_requested_ms ? requested_ms : config.default_ms;
    if (value < config.min_ms) {
        value = config.min_ms;
    }
    if (value > config.max_ms) {
        value = config.max_ms;
    }
    return value;
}

DaemonConfig load_config(const std::string& path) {
    const ConfigValues values = read_config_values(path);
    DaemonConfig config;
    config.target = read_required_string(values, "target");
    config.listen_host = read_required_string(values, "listen_host");
    config.listen_port = read_listen_port(values);
    config.default_workdir = read_required_string(values, "default_workdir");
    config.default_shell = read_optional_string(values, "default_shell", "");
    config.allow_login_shell = read_optional_bool(values, "allow_login_shell", true);
    config.http_auth_bearer_token = read_http_auth_bearer_token(values);
    config.max_request_header_bytes = read_optional_size_t(
        values,
        "max_request_header_bytes",
        64UL * 1024UL
    );
    config.max_request_body_bytes = read_optional_size_t(
        values,
        "max_request_body_bytes",
        512UL * 1024UL * 1024UL
    );
    config.transfer_limits = read_transfer_limits(values);
    config.max_open_sessions = read_optional_unsigned_long(values, "max_open_sessions", 64UL);
    config.port_forward_limits = read_port_forward_limits(values);
    config.port_forward_max_worker_threads = config.port_forward_limits.max_worker_threads;
    config.yield_time = read_yield_time_config(values);
    config.sandbox_configured = has_key_with_prefix(values, "sandbox_");
    config.sandbox = read_sandbox(values);
    validate_daemon_config(config);
    return config;
}
