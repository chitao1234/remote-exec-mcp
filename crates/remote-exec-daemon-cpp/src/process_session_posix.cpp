#ifndef _WIN32

#include <cerrno>
#include <cstring>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include <fcntl.h>
#include <signal.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "platform.h"
#include "process_session.h"

namespace {

class UniqueFd {
public:
    UniqueFd() : fd_(-1) {}
    explicit UniqueFd(int fd) : fd_(fd) {}
    ~UniqueFd() {
        reset();
    }

    UniqueFd(UniqueFd&& other) : fd_(other.release()) {}
    UniqueFd& operator=(UniqueFd&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    UniqueFd(const UniqueFd&) = delete;
    UniqueFd& operator=(const UniqueFd&) = delete;

    int get() const {
        return fd_;
    }

    bool valid() const {
        return fd_ >= 0;
    }

    int release() {
        const int released = fd_;
        fd_ = -1;
        return released;
    }

    void reset(int fd = -1) {
        if (valid()) {
            close(fd_);
        }
        fd_ = fd;
    }

private:
    int fd_;
};

struct PosixPipePair {
    UniqueFd read_end;
    UniqueFd write_end;
};

PosixPipePair create_posix_pipe(const char* label) {
    int fds[2];
    if (pipe(fds) != 0) {
        throw std::runtime_error(std::string(label) + " failed: " + std::strerror(errno));
    }
    PosixPipePair pair;
    pair.read_end.reset(fds[0]);
    pair.write_end.reset(fds[1]);
    return pair;
}

void set_nonblocking(int fd) {
    const int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0 || fcntl(fd, F_SETFL, flags | O_NONBLOCK) != 0) {
        throw std::runtime_error("fcntl(O_NONBLOCK) failed");
    }
}

std::string replacement_utf8() {
    return "\xEF\xBF\xBD";
}

bool is_continuation(unsigned char ch) {
    return (ch & 0xC0U) == 0x80U;
}

std::string decode_utf8_output(std::string* carry, const std::string& raw_chunk, bool flush) {
    std::string raw = *carry;
    raw += raw_chunk;
    carry->clear();

    std::string output;
    for (std::size_t i = 0; i < raw.size();) {
        const unsigned char ch = static_cast<unsigned char>(raw[i]);
        if (ch < 0x80U) {
            output.push_back(static_cast<char>(ch));
            ++i;
            continue;
        }

        std::size_t expected = 0;
        if (ch >= 0xC2U && ch <= 0xDFU) {
            expected = 2;
        } else if (ch >= 0xE0U && ch <= 0xEFU) {
            expected = 3;
        } else if (ch >= 0xF0U && ch <= 0xF4U) {
            expected = 4;
        } else {
            output += replacement_utf8();
            ++i;
            continue;
        }

        if (i + expected > raw.size()) {
            if (!flush) {
                carry->assign(raw.substr(i));
                break;
            }
            output += replacement_utf8();
            break;
        }

        bool valid = true;
        for (std::size_t j = 1; j < expected; ++j) {
            if (!is_continuation(static_cast<unsigned char>(raw[i + j]))) {
                valid = false;
                break;
            }
        }

        if (!valid) {
            output += replacement_utf8();
            ++i;
            continue;
        }

        output.append(raw, i, expected);
        i += expected;
    }

    return output;
}

class PosixProcessSession : public ProcessSession {
public:
    PosixProcessSession(pid_t pid, UniqueFd stdin_write, UniqueFd stdout_read)
        : pid_(pid),
          stdin_write_(std::move(stdin_write)),
          stdout_read_(std::move(stdout_read)),
          reaped_(false),
          exit_code_(0) {}

    ~PosixProcessSession() override {
        terminate();
    }

    void write_stdin(const std::string& chars) override {
        const char* data = chars.data();
        std::size_t remaining = chars.size();
        while (remaining > 0) {
            const ssize_t written = write(stdin_write_.get(), data, remaining);
            if (written < 0) {
                if (errno == EINTR) {
                    continue;
                }
                throw std::runtime_error(
                    std::string("write(stdin) failed: ") + std::strerror(errno)
                );
            }
            if (written == 0) {
                throw std::runtime_error("write(stdin) failed");
            }
            data += written;
            remaining -= static_cast<std::size_t>(written);
        }
    }

    std::string read_available(std::string* carry) override {
        std::string raw;
        char buffer[4096];
        for (;;) {
            const ssize_t read_count = read(stdout_read_.get(), buffer, sizeof(buffer));
            if (read_count > 0) {
                raw.append(buffer, static_cast<std::size_t>(read_count));
                continue;
            }
            if (read_count == 0) {
                break;
            }
            if (errno == EINTR) {
                continue;
            }
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                break;
            }
            break;
        }
        return decode_utf8_output(carry, raw, false);
    }

    std::string flush_carry(std::string* carry) override {
        return decode_utf8_output(carry, "", true);
    }

    bool has_exited(int* exit_code) override {
        if (reaped_) {
            *exit_code = exit_code_;
            return true;
        }

        int status = 0;
        const pid_t result = waitpid(pid_, &status, WNOHANG);
        if (result == 0) {
            return false;
        }
        if (result < 0) {
            if (errno == ECHILD) {
                reaped_ = true;
                exit_code_ = 1;
                *exit_code = exit_code_;
                return true;
            }
            throw std::runtime_error(std::string("waitpid failed: ") + std::strerror(errno));
        }

        reaped_ = true;
        if (WIFEXITED(status)) {
            exit_code_ = WEXITSTATUS(status);
        } else if (WIFSIGNALED(status)) {
            exit_code_ = 128 + WTERMSIG(status);
        } else {
            exit_code_ = 1;
        }
        *exit_code = exit_code_;
        return true;
    }

    void terminate() override {
        if (pid_ <= 0 || reaped_) {
            return;
        }
        kill(-pid_, SIGTERM);
        platform::sleep_ms(50);
        kill(-pid_, SIGKILL);
        int ignored_status = 0;
        waitpid(pid_, &ignored_status, 0);
        reaped_ = true;
    }

private:
    pid_t pid_;
    UniqueFd stdin_write_;
    UniqueFd stdout_read_;
    bool reaped_;
    int exit_code_;
};

}  // namespace

std::unique_ptr<ProcessSession> ProcessSession::launch(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login
) {
    PosixPipePair stdout_pipe = create_posix_pipe("pipe(stdout)");
    PosixPipePair stdin_pipe = create_posix_pipe("pipe(stdin)");
    const std::vector<std::string> argv = platform::shell_argv(shell, login, command);

    const pid_t pid = fork();
    if (pid < 0) {
        throw std::runtime_error(std::string("fork failed: ") + std::strerror(errno));
    }

    if (pid == 0) {
        setpgid(0, 0);
        dup2(stdin_pipe.read_end.get(), STDIN_FILENO);
        dup2(stdout_pipe.write_end.get(), STDOUT_FILENO);
        dup2(stdout_pipe.write_end.get(), STDERR_FILENO);

        stdin_pipe.read_end.reset();
        stdin_pipe.write_end.reset();
        stdout_pipe.read_end.reset();
        stdout_pipe.write_end.reset();

        if (!workdir.empty() && chdir(workdir.c_str()) != 0) {
            _exit(126);
        }

        setenv("LC_ALL", "C.UTF-8", 1);
        setenv("LANG", "C.UTF-8", 1);

        std::vector<char*> exec_argv;
        for (std::size_t i = 0; i < argv.size(); ++i) {
            exec_argv.push_back(const_cast<char*>(argv[i].c_str()));
        }
        exec_argv.push_back(NULL);
        execvp(exec_argv[0], &exec_argv[0]);
        _exit(127);
    }

    setpgid(pid, pid);
    stdin_pipe.read_end.reset();
    stdout_pipe.write_end.reset();
    set_nonblocking(stdout_pipe.read_end.get());

    return std::unique_ptr<ProcessSession>(
        new PosixProcessSession(
            pid,
            std::move(stdin_pipe.write_end),
            std::move(stdout_pipe.read_end)
        )
    );
}

#endif
