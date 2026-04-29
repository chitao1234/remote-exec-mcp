#include <cstdlib>
#include <cerrno>
#include <climits>
#include <fstream>
#include <map>
#include <sstream>
#include <stdexcept>

#include "config.h"
#include "text_utils.h"

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
    const std::map<std::string, std::string>& values,
    const std::string& key,
    unsigned long fallback
) {
    const std::map<std::string, std::string>::const_iterator it = values.find(key);
    if (it == values.end()) {
        return fallback;
    }
    return parse_unsigned_long(it->second, key);
}

static std::string read_required_string(
    const std::map<std::string, std::string>& values,
    const std::string& key
) {
    const std::map<std::string, std::string>::const_iterator it = values.find(key);
    if (it == values.end()) {
        throw std::runtime_error("missing required config key: " + key);
    }
    return it->second;
}

static std::string read_optional_string(
    const std::map<std::string, std::string>& values,
    const std::string& key,
    const std::string& fallback
) {
    const std::map<std::string, std::string>::const_iterator it = values.find(key);
    return it == values.end() ? fallback : it->second;
}

static bool read_optional_bool(
    const std::map<std::string, std::string>& values,
    const std::string& key,
    bool fallback
) {
    const std::map<std::string, std::string>::const_iterator it = values.find(key);
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
    const std::map<std::string, std::string>& values,
    const std::string& key,
    std::size_t fallback
) {
    const std::map<std::string, std::string>::const_iterator it = values.find(key);
    if (it == values.end()) {
        return fallback;
    }
    const unsigned long value = parse_unsigned_long(it->second, key);
    return static_cast<std::size_t>(value);
}

static bool contains_ascii_whitespace(const std::string& value) {
    return value.find_first_of(" \t\r\n") != std::string::npos;
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
    const std::map<std::string, std::string>& values,
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
    std::ifstream input(path.c_str());
    if (!input) {
        throw std::runtime_error("unable to open config file: " + path);
    }

    std::map<std::string, std::string> values;
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

    DaemonConfig config;
    config.target = read_required_string(values, "target");
    config.listen_host = read_required_string(values, "listen_host");
    {
        const unsigned long listen_port =
            parse_unsigned_long(read_required_string(values, "listen_port"), "listen_port");
        if (listen_port == 0 || listen_port > 65535UL) {
            throw std::runtime_error("listen_port must be between 1 and 65535");
        }
        config.listen_port = static_cast<int>(listen_port);
    }
    config.default_workdir = read_required_string(values, "default_workdir");
    config.default_shell = read_optional_string(values, "default_shell", "");
    config.allow_login_shell = read_optional_bool(values, "allow_login_shell", true);
    config.http_auth_bearer_token.clear();
    {
        const std::map<std::string, std::string>::const_iterator it =
            values.find("http_auth_bearer_token");
        if (it != values.end()) {
            if (it->second.empty()) {
                throw std::runtime_error("http_auth_bearer_token must not be empty");
            }
            if (contains_ascii_whitespace(it->second)) {
                throw std::runtime_error("http_auth_bearer_token must not contain whitespace");
            }
            config.http_auth_bearer_token = it->second;
        }
    }
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
    config.max_open_sessions = read_optional_unsigned_long(values, "max_open_sessions", 64UL);
    if (config.max_request_header_bytes == 0) {
        throw std::runtime_error("max_request_header_bytes must be greater than zero");
    }
    if (config.max_request_body_bytes == 0) {
        throw std::runtime_error("max_request_body_bytes must be greater than zero");
    }
    if (config.max_open_sessions == 0) {
        throw std::runtime_error("max_open_sessions must be greater than zero");
    }
    config.yield_time = default_yield_time_config();
    config.yield_time.exec_command = read_yield_time_operation(
        values,
        "yield_time_exec_command",
        config.yield_time.exec_command
    );
    config.yield_time.write_stdin_poll = read_yield_time_operation(
        values,
        "yield_time_write_stdin_poll",
        config.yield_time.write_stdin_poll
    );
    config.yield_time.write_stdin_input = read_yield_time_operation(
        values,
        "yield_time_write_stdin_input",
        config.yield_time.write_stdin_input
    );
    return config;
}
