#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <sstream>
#include <string>
#include <vector>

#include <windows.h>

#include "logging.h"

static std::string trim(const std::string& raw) {
    const std::string whitespace = " \t\r\n";
    const std::size_t start = raw.find_first_not_of(whitespace);
    if (start == std::string::npos) {
        return "";
    }
    const std::size_t end = raw.find_last_not_of(whitespace);
    return raw.substr(start, end - start + 1);
}

static std::string lowercase(std::string value) {
    std::transform(
        value.begin(),
        value.end(),
        value.begin(),
        [](unsigned char ch) { return static_cast<char>(std::tolower(ch)); }
    );
    return value;
}

static bool parse_level_token(const std::string& raw, LogLevel* level) {
    const std::string token = lowercase(trim(raw));
    if (token == "trace") {
        *level = LOG_TRACE;
        return true;
    }
    if (token == "debug") {
        *level = LOG_DEBUG;
        return true;
    }
    if (token == "info") {
        *level = LOG_INFO;
        return true;
    }
    if (token == "warn" || token == "warning") {
        *level = LOG_WARN;
        return true;
    }
    if (token == "error") {
        *level = LOG_ERROR;
        return true;
    }
    if (token == "off") {
        *level = LOG_OFF;
        return true;
    }
    return false;
}

static std::vector<std::string> split_filters(const std::string& raw) {
    std::vector<std::string> parts;
    std::string current;
    for (std::size_t index = 0; index < raw.size(); ++index) {
        const char ch = raw[index];
        if (ch == ',') {
            parts.push_back(current);
            current.clear();
        } else {
            current.push_back(ch);
        }
    }
    parts.push_back(current);
    return parts;
}

static LogLevel parse_filter_value(const char* raw) {
    LogLevel default_level = LOG_INFO;
    bool has_component_level = false;
    LogLevel component_level = LOG_INFO;
    const std::vector<std::string> parts = split_filters(raw);

    for (std::size_t index = 0; index < parts.size(); ++index) {
        const std::string token = trim(parts[index]);
        const std::size_t equals = token.find('=');
        if (equals == std::string::npos) {
            LogLevel parsed = LOG_INFO;
            if (parse_level_token(token, &parsed)) {
                default_level = parsed;
            }
            continue;
        }

        const std::string key = lowercase(trim(token.substr(0, equals)));
        const std::string value = token.substr(equals + 1);
        if (key == "remote_exec_daemon_xp" || key == "daemon_xp") {
            LogLevel parsed = LOG_INFO;
            if (parse_level_token(value, &parsed)) {
                component_level = parsed;
                has_component_level = true;
            }
        }
    }

    return has_component_level ? component_level : default_level;
}

static LogLevel configured_level() {
    static bool initialized = false;
    static LogLevel level = LOG_INFO;

    if (!initialized) {
        const char* raw = std::getenv("REMOTE_EXEC_LOG");
        if (raw == NULL || raw[0] == '\0') {
            raw = std::getenv("RUST_LOG");
        }
        if (raw != NULL && raw[0] != '\0') {
            level = parse_filter_value(raw);
        }
        initialized = true;
    }

    return level;
}

static const char* level_name(LogLevel level) {
    switch (level) {
    case LOG_TRACE:
        return "TRACE";
    case LOG_DEBUG:
        return "DEBUG";
    case LOG_INFO:
        return "INFO";
    case LOG_WARN:
        return "WARN";
    case LOG_ERROR:
        return "ERROR";
    case LOG_OFF:
        return "OFF";
    }
    return "INFO";
}

void init_logging() {
    (void)configured_level();
}

bool log_enabled(LogLevel level) {
    const LogLevel configured = configured_level();
    if (configured == LOG_OFF) {
        return false;
    }
    return static_cast<int>(level) >= static_cast<int>(configured);
}

void log_message(LogLevel level, const std::string& component, const std::string& message) {
    if (!log_enabled(level)) {
        return;
    }

    SYSTEMTIME now;
    GetLocalTime(&now);
    std::fprintf(
        stderr,
        "%04d-%02d-%02d %02d:%02d:%02d.%03d %-5s %s %s\n",
        static_cast<int>(now.wYear),
        static_cast<int>(now.wMonth),
        static_cast<int>(now.wDay),
        static_cast<int>(now.wHour),
        static_cast<int>(now.wMinute),
        static_cast<int>(now.wSecond),
        static_cast<int>(now.wMilliseconds),
        level_name(level),
        component.c_str(),
        message.c_str()
    );
    std::fflush(stderr);
}

std::string preview_text(const std::string& text, std::size_t limit) {
    if (text.size() <= limit) {
        return text;
    }
    return text.substr(0, limit) + "...";
}
