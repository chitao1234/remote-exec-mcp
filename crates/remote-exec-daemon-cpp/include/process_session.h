#pragma once

#include <memory>
#include <string>

class ProcessSession {
public:
    virtual ~ProcessSession() {}

    ProcessSession(const ProcessSession&) = delete;
    ProcessSession& operator=(const ProcessSession&) = delete;

    static std::unique_ptr<ProcessSession> launch(
        const std::string& command,
        const std::string& workdir,
        const std::string& shell,
        bool login
    );

    virtual void write_stdin(const std::string& chars) = 0;
    virtual std::string read_available(std::string* carry) = 0;
    virtual std::string flush_carry(std::string* carry) = 0;
    virtual bool has_exited(int* exit_code) = 0;
    virtual void terminate() = 0;

protected:
    ProcessSession() {}
};
