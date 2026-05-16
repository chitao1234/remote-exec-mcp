#ifndef _WIN32

#include <atomic>
#include <cerrno>
#include <cstring>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

#include "platform.h"
#include "posix_child_reaper.h"
#include "process_session.h"

extern char** environ;

namespace {

const unsigned short DEFAULT_PTY_ROWS = 24;
const unsigned short DEFAULT_PTY_COLS = 120;

// strerror_r has different return types on GNU (char*) vs POSIX XSI (int).
// These overloads let the compiler pick the right handler.
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wunused-function"
static std::string strerror_result(int ret, char* buf, int errnum) {
    if (ret == 0) return std::string(buf);
    return "errno " + std::to_string(errnum);
}

static std::string strerror_result(char* ret, char*, int) {
    return std::string(ret);
}
#pragma GCC diagnostic pop

static std::string safe_strerror(int errnum) {
    char buf[256];
    buf[0] = '\0';
    return strerror_result(strerror_r(errnum, buf, sizeof(buf)), buf, errnum);
}
// Grace period for cooperative shutdown before escalating from SIGTERM to SIGKILL.
const int TERMINATE_GRACE_MS = 50;
const std::size_t PROCESS_OUTPUT_READ_BUFFER_SIZE = 4U * 1024U;

#ifdef REMOTE_EXEC_CPP_TESTING
std::atomic<unsigned long> g_test_exit_poll_delay_ms(0UL);

void maybe_apply_test_exit_poll_delay() {
    const unsigned long delay_ms = g_test_exit_poll_delay_ms.load();
    if (delay_ms > 0UL) {
        platform::sleep_ms(delay_ms);
    }
}
#else
void maybe_apply_test_exit_poll_delay() {
}
#endif

class UniqueFd {
public:
    UniqueFd() : fd_(-1) {}
    explicit UniqueFd(int fd) : fd_(fd) {}
    ~UniqueFd() { reset(); }

    UniqueFd(UniqueFd&& other) : fd_(other.release()) {}
    UniqueFd& operator=(UniqueFd&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    UniqueFd(const UniqueFd&) = delete;
    UniqueFd& operator=(const UniqueFd&) = delete;

    int get() const { return fd_; }

    bool valid() const { return fd_ >= 0; }

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

struct PosixPtyPair {
    UniqueFd master;
    std::string slave_path;
};

PosixPipePair create_posix_pipe(const char* label) {
    int fds[2];
#ifdef __linux__
    if (pipe2(fds, O_CLOEXEC) != 0) {
        throw std::runtime_error(std::string(label) + " failed: " + safe_strerror(errno));
    }
#else
    if (pipe(fds) != 0) {
        throw std::runtime_error(std::string(label) + " failed: " + safe_strerror(errno));
    }
    try {
        const int read_flags = fcntl(fds[0], F_GETFD, 0);
        if (read_flags < 0) {
            throw std::runtime_error(std::string(label) + " fcntl(F_GETFD) failed: " + safe_strerror(errno));
        }
        if (fcntl(fds[0], F_SETFD, read_flags | FD_CLOEXEC) != 0) {
            throw std::runtime_error(std::string(label) + " fcntl(F_SETFD) failed: " + safe_strerror(errno));
        }
        const int write_flags = fcntl(fds[1], F_GETFD, 0);
        if (write_flags < 0) {
            throw std::runtime_error(std::string(label) + " fcntl(F_GETFD) failed: " + safe_strerror(errno));
        }
        if (fcntl(fds[1], F_SETFD, write_flags | FD_CLOEXEC) != 0) {
            throw std::runtime_error(std::string(label) + " fcntl(F_SETFD) failed: " + safe_strerror(errno));
        }
    } catch (...) {
        close(fds[0]);
        close(fds[1]);
        throw;
    }
#endif
    PosixPipePair pair;
    pair.read_end.reset(fds[0]);
    pair.write_end.reset(fds[1]);
    return pair;
}

UniqueFd open_dev_null_read() {
    UniqueFd fd(open("/dev/null", O_RDONLY | O_CLOEXEC));
    if (!fd.valid()) {
        throw std::runtime_error(std::string("open(/dev/null) failed: ") + safe_strerror(errno));
    }
    return fd;
}

void kill_process_group(pid_t pid) {
    kill(-pid, SIGTERM);
    platform::sleep_ms(TERMINATE_GRACE_MS);
    kill(-pid, SIGKILL);
}

PosixPtyPair create_posix_pty() {
    UniqueFd master(posix_openpt(O_RDWR | O_NOCTTY));
    if (!master.valid()) {
        throw std::runtime_error(std::string("posix_openpt failed: ") + safe_strerror(errno));
    }
    if (grantpt(master.get()) != 0) {
        throw std::runtime_error(std::string("grantpt failed: ") + safe_strerror(errno));
    }
    if (unlockpt(master.get()) != 0) {
        throw std::runtime_error(std::string("unlockpt failed: ") + safe_strerror(errno));
    }

#ifdef __GLIBC__
    char pts_buf[256];
    if (ptsname_r(master.get(), pts_buf, sizeof(pts_buf)) != 0) {
        throw std::runtime_error(std::string("ptsname_r failed: ") + safe_strerror(errno));
    }
    std::string slave_path(pts_buf);
#else
    char* slave_name = ptsname(master.get());
    if (slave_name == nullptr) {
        throw std::runtime_error(std::string("ptsname failed: ") + safe_strerror(errno));
    }
    std::string slave_path(slave_name);
#endif

    struct winsize size;
    std::memset(&size, 0, sizeof(size));
    size.ws_row = DEFAULT_PTY_ROWS;
    size.ws_col = DEFAULT_PTY_COLS;
    if (ioctl(master.get(), TIOCSWINSZ, &size) != 0) {
        throw std::runtime_error(std::string("ioctl(TIOCSWINSZ) failed: ") + safe_strerror(errno));
    }

    PosixPtyPair pair;
    pair.master = std::move(master);
    pair.slave_path = slave_path;
    return pair;
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

bool readable_now(int fd) {
    struct pollfd descriptor;
    descriptor.fd = fd;
    descriptor.events = POLLIN | POLLHUP | POLLERR;
    descriptor.revents = 0;

    for (;;) {
        const int result = poll(&descriptor, 1, 0);
        if (result > 0) {
            return (descriptor.revents & (POLLIN | POLLHUP | POLLERR)) != 0;
        }
        if (result == 0) {
            return false;
        }
        if (errno != EINTR) {
            throw std::runtime_error(std::string("poll failed: ") + safe_strerror(errno));
        }
    }
}

void wait_until_readable(int fd) {
    struct pollfd descriptor;
    descriptor.fd = fd;
    descriptor.events = POLLIN | POLLHUP | POLLERR;
    descriptor.revents = 0;

    for (;;) {
        const int result = poll(&descriptor, 1, -1);
        if (result > 0) {
            return;
        }
        if (result < 0 && errno == EINTR) {
            continue;
        }
        if (result < 0) {
            throw std::runtime_error(std::string("poll failed: ") + safe_strerror(errno));
        }
    }
}

struct ExecEnvironment {
    std::vector<std::string> values;
    std::vector<char*> pointers;

    void refresh_pointers() {
        pointers.clear();
        pointers.reserve(values.size() + 1U);
        for (std::size_t i = 0; i < values.size(); ++i) {
            pointers.push_back(const_cast<char*>(values[i].c_str()));
        }
        pointers.push_back(nullptr);
    }
};

bool env_key_matches(const std::string& entry, const char* key) {
    const std::size_t key_len = std::strlen(key);
    return entry.size() > key_len && entry.compare(0, key_len, key) == 0 && entry[key_len] == '=';
}

void upsert_env_value(std::vector<std::string>* values, const std::string& assignment) {
    const std::size_t equals = assignment.find('=');
    const std::string key = equals == std::string::npos ? assignment : assignment.substr(0, equals);
    for (std::size_t i = 0; i < values->size(); ++i) {
        if (env_key_matches((*values)[i], key.c_str())) {
            (*values)[i] = assignment;
            return;
        }
    }
    values->push_back(assignment);
}

ExecEnvironment build_exec_environment_values(bool tty) {
    ExecEnvironment env;
    for (char** current = environ; current != nullptr && *current != nullptr; ++current) {
        env.values.push_back(*current);
    }
    upsert_env_value(&env.values, "LC_ALL=C.UTF-8");
    upsert_env_value(&env.values, "LANG=C.UTF-8");
    if (tty) {
        bool has_term = false;
        for (std::size_t i = 0; i < env.values.size(); ++i) {
            if (env_key_matches(env.values[i], "TERM")) {
                has_term = true;
                break;
            }
        }
        if (!has_term) {
            env.values.push_back("TERM=xterm-256color");
        }
    }
    return env;
}

bool is_path_like_command(const std::string& command) {
    return command.find('/') != std::string::npos;
}

std::string path_env_from(const ExecEnvironment& env) {
    for (std::size_t i = 0; i < env.values.size(); ++i) {
        if (env_key_matches(env.values[i], "PATH")) {
            return env.values[i].substr(5);
        }
    }
    return "/bin:/usr/bin";
}

std::string resolve_exec_path(const std::string& program, const ExecEnvironment& env) {
    if (program.empty() || is_path_like_command(program)) {
        return program;
    }

    const std::string path = path_env_from(env);
    std::string current;
    for (std::size_t i = 0; i <= path.size(); ++i) {
        if (i != path.size() && path[i] != ':') {
            current.push_back(path[i]);
            continue;
        }
        const std::string dir = current.empty() ? "." : current;
        const std::string candidate = dir + "/" + program;
        if (access(candidate.c_str(), X_OK) == 0) {
            return candidate;
        }
        current.clear();
    }
    return program;
}

std::vector<char*> build_exec_argv(const std::vector<std::string>& argv) {
    std::vector<char*> exec_argv;
    exec_argv.reserve(argv.size() + 1U);
    for (std::size_t i = 0; i < argv.size(); ++i) {
        exec_argv.push_back(const_cast<char*>(argv[i].c_str()));
    }
    exec_argv.push_back(nullptr);
    return exec_argv;
}

void exec_shell_child(const std::vector<char*>& exec_argv,
                      const std::string& executable_path,
                      const ExecEnvironment& environment,
                      const std::string& workdir) {
    signal(SIGPIPE, SIG_DFL);
    if (!workdir.empty() && chdir(workdir.c_str()) != 0) {
        _exit(126);
    }

    execve(executable_path.c_str(),
           const_cast<char* const*>(&exec_argv[0]),
           const_cast<char* const*>(&environment.pointers[0]));
    _exit(127);
}

void record_exit_status(int status, int* exit_code) {
    if (WIFEXITED(status)) {
        *exit_code = WEXITSTATUS(status);
    } else if (WIFSIGNALED(status)) {
        *exit_code = 128 + WTERMSIG(status);
    } else {
        *exit_code = 1;
    }
}

class PosixProcessSession : public ProcessSession {
public:
    PosixProcessSession(pid_t pid, bool tty, UniqueFd input_write, UniqueFd output_read)
        : pid_(pid), tty_(tty), input_write_(std::move(input_write)), output_read_(std::move(output_read)),
          reaped_(false), exit_code_(0) {
        register_posix_child(pid_);
    }

    ~PosixProcessSession() override { terminate(); }

    void write_stdin(const std::string& chars) override {
        if (!input_write_.valid()) {
            throw std::runtime_error(
                "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open");
        }

        const char* data = chars.data();
        std::size_t remaining = chars.size();
        while (remaining > 0) {
            const ssize_t written = write(input_write_.get(), data, remaining);
            if (written < 0) {
                if (errno == EINTR) {
                    continue;
                }
                if (errno == EPIPE || errno == EIO) {
                    throw ProcessStdinClosedError(
                        "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open");
                }
                throw std::runtime_error(std::string("write(stdin) failed: ") + safe_strerror(errno));
            }
            if (written == 0) {
                throw std::runtime_error("write(stdin) failed");
            }
            data += written;
            remaining -= static_cast<std::size_t>(written);
        }
    }

    void resize_pty(unsigned short rows, unsigned short cols) override {
        if (!tty_ || !input_write_.valid()) {
            throw ProcessPtyResizeUnsupportedError("PTY resize requires a tty session");
        }
        if (rows == 0U || cols == 0U) {
            throw ProcessPtyResizeUnsupportedError("PTY rows and cols must be greater than zero");
        }
        struct winsize size;
        std::memset(&size, 0, sizeof(size));
        size.ws_row = rows;
        size.ws_col = cols;
        if (ioctl(input_write_.get(), TIOCSWINSZ, &size) != 0) {
            throw std::runtime_error(std::string("ioctl(TIOCSWINSZ) failed: ") + safe_strerror(errno));
        }
    }

    std::string read_output(bool block, bool* eof, std::string* carry) override {
        *eof = false;
        std::string raw;
        char buffer[PROCESS_OUTPUT_READ_BUFFER_SIZE];
        const int read_fd = output_read_.valid() ? output_read_.get() : input_write_.get();
        if (block && !readable_now(read_fd)) {
            wait_until_readable(read_fd);
        }
        while (block || readable_now(read_fd)) {
            const ssize_t read_count = read(read_fd, buffer, sizeof(buffer));
            if (read_count > 0) {
                raw.append(buffer, static_cast<std::size_t>(read_count));
                block = false;
                continue;
            }
            if (read_count == 0) {
                *eof = true;
                break;
            }
            if (errno == EINTR) {
                continue;
            }
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                break;
            }
            if (errno == EIO) {
                *eof = true;
                break;
            }
            throw std::runtime_error(std::string("read(stdout) failed: ") + safe_strerror(errno));
        }
        return decode_utf8_output(carry, raw, false);
    }

    std::string flush_carry(std::string* carry) override { return decode_utf8_output(carry, "", true); }

    bool has_exited(int* exit_code) override {
        if (reaped_) {
            *exit_code = exit_code_;
            return true;
        }

        maybe_apply_test_exit_poll_delay();
        int status = 0;
        if (poll_posix_child_exit(pid_, &status)) {
            reaped_ = true;
            record_exit_status(status, &exit_code_);
            *exit_code = exit_code_;
            return true;
        }
        return false;
    }

    void terminate() override {
        if (pid_ <= 0 || reaped_) {
            return;
        }
        kill_process_group(pid_);
        int ignored_status = 0;
        if (wait_posix_child_exit(pid_, &ignored_status)) {
            reaped_ = true;
        }
        return;
    }

    bool terminate_descendants() override {
        if (pid_ > 0) {
            kill_process_group(pid_);
            return true;
        }
        return false;
    }

private:
    pid_t pid_;
    bool tty_;
    UniqueFd input_write_;
    UniqueFd output_read_;
    bool reaped_;
    int exit_code_;
};

} // namespace

bool process_session_supports_pty() {
    static const bool supported = []() {
        try {
            PosixPtyPair pair = create_posix_pty();
            return pair.master.valid() && !pair.slave_path.empty();
        } catch (const std::exception&) {
            return false;
        }
    }();
    return supported;
}

std::unique_ptr<ProcessSession> ProcessSession::launch(
    const std::string& command, const std::string& workdir, const std::string& shell, bool login, bool tty) {
    const std::vector<std::string> argv = platform::shell_argv(shell, login, command);
    ExecEnvironment exec_environment = build_exec_environment_values(tty);
    exec_environment.refresh_pointers();
    const std::vector<char*> exec_argv = build_exec_argv(argv);
    const std::string executable_path = resolve_exec_path(argv[0], exec_environment);

    if (tty) {
        PosixPtyPair pty = create_posix_pty();
        const pid_t pid = fork();
        if (pid < 0) {
            throw std::runtime_error(std::string("fork failed: ") + safe_strerror(errno));
        }

        if (pid == 0) {
            setsid();
            const int slave_fd = open(pty.slave_path.c_str(), O_RDWR);
            if (slave_fd < 0) {
                _exit(126);
            }
#ifdef TIOCSCTTY
            ioctl(slave_fd, TIOCSCTTY, 0);
#endif
            dup2(slave_fd, STDIN_FILENO);
            dup2(slave_fd, STDOUT_FILENO);
            dup2(slave_fd, STDERR_FILENO);
            if (slave_fd > STDERR_FILENO) {
                close(slave_fd);
            }
            pty.master.reset();
            exec_shell_child(exec_argv, executable_path, exec_environment, workdir);
        }

        return std::unique_ptr<ProcessSession>(new PosixProcessSession(pid, true, std::move(pty.master), UniqueFd()));
    }

    PosixPipePair stdout_pipe = create_posix_pipe("pipe(stdout)");
    UniqueFd stdin_null = open_dev_null_read();

    const pid_t pid = fork();
    if (pid < 0) {
        throw std::runtime_error(std::string("fork failed: ") + safe_strerror(errno));
    }

    if (pid == 0) {
        if (setpgid(0, 0) != 0) {
            _exit(126);
        }
        if (dup2(stdin_null.get(), STDIN_FILENO) < 0 || dup2(stdout_pipe.write_end.get(), STDOUT_FILENO) < 0 ||
            dup2(stdout_pipe.write_end.get(), STDERR_FILENO) < 0) {
            _exit(126);
        }

        stdin_null.reset();
        stdout_pipe.read_end.reset();
        stdout_pipe.write_end.reset();
        exec_shell_child(exec_argv, executable_path, exec_environment, workdir);
    }

    setpgid(pid, pid);
    stdin_null.reset();
    stdout_pipe.write_end.reset();

    return std::unique_ptr<ProcessSession>(
        new PosixProcessSession(pid, false, UniqueFd(), std::move(stdout_pipe.read_end)));
}

#ifdef REMOTE_EXEC_CPP_TESTING
void set_process_session_test_exit_poll_delay_ms(unsigned long delay_ms) {
    g_test_exit_poll_delay_ms.store(delay_ms);
}
#endif

#endif
