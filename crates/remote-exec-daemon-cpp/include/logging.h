#pragma once

#include <cstddef>
#include <string>

enum LogLevel {
    LOG_TRACE = 0,
    LOG_DEBUG = 1,
    LOG_INFO = 2,
    LOG_WARN = 3,
    LOG_ERROR = 4,
    LOG_OFF = 5,
};

void init_logging();
bool log_enabled(LogLevel level);
void log_message(LogLevel level, const std::string& component, const std::string& message);
std::string preview_text(const std::string& text, std::size_t limit);
