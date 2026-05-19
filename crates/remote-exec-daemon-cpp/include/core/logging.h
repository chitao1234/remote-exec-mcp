#pragma once

#include <cstddef>
#include <sstream>
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

class LogMessageBuilder {
public:
    explicit LogMessageBuilder(const std::string& prefix);

    LogMessageBuilder& raw(const std::string& token);
    LogMessageBuilder& quoted_field(const std::string& name, const std::string& value);
    LogMessageBuilder& bool_field(const std::string& name, bool value);

    template <typename T>
    LogMessageBuilder& field(const std::string& name, const T& value) {
        append_separator();
        out_ << name << "=" << value;
        return *this;
    }

    std::string str() const;

private:
    void append_separator();

    std::ostringstream out_;
    bool needs_separator_;
};
