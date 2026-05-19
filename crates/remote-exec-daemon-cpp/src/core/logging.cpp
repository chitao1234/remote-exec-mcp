#include <algorithm>
#include <cctype>
#include <cstdio>
#include <cstdlib>
#include <ctime>
#include <sstream>
#include <string>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#else
#include <sys/time.h>
#endif

#include "logging.h"
#include "text_utils.h"

static bool parse_level_token(const std::string& raw, LogLevel* level) {
    const std::string token = lowercase_ascii(trim_ascii(raw));
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
        const std::string token = trim_ascii(parts[index]);
        const std::size_t equals = token.find('=');
        if (equals == std::string::npos) {
            LogLevel parsed = LOG_INFO;
            if (parse_level_token(token, &parsed)) {
                default_level = parsed;
            }
            continue;
        }

        const std::string key = lowercase_ascii(trim_ascii(token.substr(0, equals)));
        const std::string value = token.substr(equals + 1);
        if (key == "remote_exec_daemon_cpp" || key == "daemon_cpp" || key == "remote_exec_daemon_xp" ||
            key == "daemon_xp") {
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
        if (raw == nullptr || raw[0] == '\0') {
            raw = std::getenv("RUST_LOG");
        }
        if (raw != nullptr && raw[0] != '\0') {
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

#ifdef _WIN32
    SYSTEMTIME now;
    GetLocalTime(&now);
    const int year = static_cast<int>(now.wYear);
    const int month = static_cast<int>(now.wMonth);
    const int day = static_cast<int>(now.wDay);
    const int hour = static_cast<int>(now.wHour);
    const int minute = static_cast<int>(now.wMinute);
    const int second = static_cast<int>(now.wSecond);
    const int millisecond = static_cast<int>(now.wMilliseconds);
#else
    struct timeval tv;
    gettimeofday(&tv, nullptr);
    struct tm local_time;
    localtime_r(&tv.tv_sec, &local_time);
    const int year = local_time.tm_year + 1900;
    const int month = local_time.tm_mon + 1;
    const int day = local_time.tm_mday;
    const int hour = local_time.tm_hour;
    const int minute = local_time.tm_min;
    const int second = local_time.tm_sec;
    const int millisecond = static_cast<int>(tv.tv_usec / 1000);
#endif

    std::fprintf(stderr,
                 "%04d-%02d-%02d %02d:%02d:%02d.%03d %-5s %s %s\n",
                 year,
                 month,
                 day,
                 hour,
                 minute,
                 second,
                 millisecond,
                 level_name(level),
                 component.c_str(),
                 message.c_str());
    std::fflush(stderr);
}

std::string preview_text(const std::string& text, std::size_t limit) {
    if (text.size() <= limit) {
        return text;
    }
    return text.substr(0, limit) + "...";
}

LogMessageBuilder::LogMessageBuilder(const std::string& prefix) : needs_separator_(false) {
    if (!prefix.empty()) {
        out_ << prefix;
        needs_separator_ = true;
    }
}

LogMessageBuilder& LogMessageBuilder::raw(const std::string& token) {
    append_separator();
    out_ << token;
    return *this;
}

LogMessageBuilder& LogMessageBuilder::quoted_field(const std::string& name, const std::string& value) {
    append_separator();
    out_ << name << "=`" << value << "`";
    return *this;
}

LogMessageBuilder& LogMessageBuilder::bool_field(const std::string& name, bool value) {
    append_separator();
    out_ << name << "=" << (value ? "true" : "false");
    return *this;
}

std::string LogMessageBuilder::str() const {
    return out_.str();
}

void LogMessageBuilder::append_separator() {
    if (needs_separator_) {
        out_ << ' ';
    } else {
        needs_separator_ = true;
    }
}
