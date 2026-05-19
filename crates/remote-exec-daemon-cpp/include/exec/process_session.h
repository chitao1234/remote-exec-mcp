#pragma once

#include <memory>
#include <stdexcept>
#include <string>

class ProcessStdinClosedError : public std::runtime_error {
public:
    explicit ProcessStdinClosedError(const std::string& message) : std::runtime_error(message) {}
};

class ProcessPtyResizeUnsupportedError : public std::runtime_error {
public:
    explicit ProcessPtyResizeUnsupportedError(const std::string& message) : std::runtime_error(message) {}
};

class ProcessSession {
public:
    virtual ~ProcessSession() {}

    ProcessSession(const ProcessSession&) = delete;
    ProcessSession& operator=(const ProcessSession&) = delete;

    static std::unique_ptr<ProcessSession>
    launch(const std::string& command, const std::string& workdir, const std::string& shell, bool login, bool tty);

    virtual void write_stdin(const std::string& chars) = 0;
    virtual void resize_pty(unsigned short rows, unsigned short cols) = 0;
    virtual std::string read_output(bool block, bool* eof, std::string* carry) = 0;
    virtual std::string flush_carry(std::string* carry) = 0;
    virtual bool has_exited(int* exit_code) = 0;
    virtual void terminate() = 0;
    virtual bool terminate_descendants() { return false; }

protected:
    ProcessSession() {}
};

bool process_session_supports_pty();

#if !defined(_WIN32) && defined(REMOTE_EXEC_CPP_TESTING)
void set_process_session_test_exit_poll_delay_ms(unsigned long delay_ms);
#endif
