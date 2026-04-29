#include <algorithm>
#include <atomic>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <stdexcept>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <fcntl.h>
#include <signal.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

#include "logging.h"
#include "platform.h"
#include "session_store.h"

#ifdef _WIN32
#include "console_output.h"
#include "win32_error.h"
#include "win32_scoped.h"
#endif

namespace {

std::atomic<unsigned long> next_id(1UL);

std::string make_chunk_id() {
    std::ostringstream out;
    out << "cpp-" << platform::monotonic_ms() << '-' << next_id.fetch_add(1UL);
    return out.str();
}

struct PollResult {
    std::string output;
    bool completed;
    int exit_code;
};

std::string trim_output(const std::string& output, unsigned long max_output_chars) {
    if (max_output_chars == 0 || output.size() <= max_output_chars) {
        return output;
    }
    return output.substr(output.size() - max_output_chars);
}

double wall_time_seconds(std::uint64_t started_at_ms) {
    const std::uint64_t now = platform::monotonic_ms();
    if (now < started_at_ms) {
        return 0.0;
    }
    return static_cast<double>(now - started_at_ms) / 1000.0;
}

Json build_response(
    const char* daemon_session_id,
    bool running,
    std::uint64_t started_at_ms,
    bool has_exit_code,
    int exit_code,
    const std::string& output,
    unsigned long max_output_chars
) {
    const std::string trimmed = trim_output(output, max_output_chars);
    unsigned long original_token_count = 0;
    {
        std::istringstream tokens(output);
        std::string token;
        while (tokens >> token) {
            ++original_token_count;
        }
    }
    return Json{
        {"daemon_session_id", daemon_session_id != NULL ? Json(daemon_session_id) : Json(nullptr)},
        {"running", running},
        {"chunk_id", make_chunk_id()},
        {"wall_time_seconds", wall_time_seconds(started_at_ms)},
        {"exit_code", has_exit_code ? Json(exit_code) : Json(nullptr)},
        {"original_token_count", original_token_count},
        {"output", trimmed},
        {"warnings", Json::array()}
    };
}

#ifdef _WIN32
std::string windows_quote_arg(const std::string& arg) {
    if (arg.empty()) {
        return "\"\"";
    }

    bool needs_quotes = false;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        if (arg[i] == ' ' || arg[i] == '\t' || arg[i] == '"') {
            needs_quotes = true;
            break;
        }
    }
    if (!needs_quotes) {
        return arg;
    }

    std::string quoted = "\"";
    std::size_t backslashes = 0;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        const char ch = arg[i];
        if (ch == '\\') {
            ++backslashes;
            continue;
        }
        if (ch == '"') {
            quoted.append(backslashes * 2U + 1U, '\\');
            quoted.push_back('"');
            backslashes = 0;
            continue;
        }
        quoted.append(backslashes, '\\');
        backslashes = 0;
        quoted.push_back(ch);
    }
    quoted.append(backslashes * 2U, '\\');
    quoted.push_back('"');
    return quoted;
}

std::string command_line_from_argv(const std::vector<std::string>& argv) {
    std::ostringstream out;
    for (std::size_t i = 0; i < argv.size(); ++i) {
        if (i != 0) {
            out << ' ';
        }
        out << windows_quote_arg(argv[i]);
    }
    return out.str();
}
#endif

#ifndef _WIN32
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
#endif

}  // namespace

class ProcessSession {
public:
    ProcessSession() {}
    ~ProcessSession() {
        terminate();
    }

    ProcessSession(const ProcessSession&) = delete;
    ProcessSession& operator=(const ProcessSession&) = delete;

    static std::unique_ptr<ProcessSession> launch(
        const std::string& command,
        const std::string& workdir,
        const std::string& shell,
        bool login
    );

    void write_stdin(const std::string& chars);
    std::string read_available(std::string* carry);
    std::string flush_carry(std::string* carry);
    bool has_exited(int* exit_code);
    void terminate();

private:
#ifdef _WIN32
    UniqueHandle process_handle_;
    UniqueHandle stdin_write_;
    UniqueHandle stdout_read_;
#else
    pid_t pid_ = -1;
    UniqueFd stdin_write_;
    UniqueFd stdout_read_;
    bool reaped_ = false;
    int exit_code_ = 0;
#endif
};

#ifdef _WIN32
struct PipePair {
    UniqueHandle read_end;
    UniqueHandle write_end;
};

PipePair create_pipe_pair(const char* label) {
    SECURITY_ATTRIBUTES sa;
    sa.nLength = sizeof(sa);
    sa.lpSecurityDescriptor = NULL;
    sa.bInheritHandle = TRUE;

    HANDLE read_end = NULL;
    HANDLE write_end = NULL;
    if (CreatePipe(&read_end, &write_end, &sa, 0) == 0) {
        throw std::runtime_error(last_error_message(label));
    }

    PipePair pair;
    pair.read_end.reset(read_end);
    pair.write_end.reset(write_end);
    return pair;
}

std::unique_ptr<ProcessSession> ProcessSession::launch(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login
) {
    PipePair stdout_pipe = create_pipe_pair("CreatePipe(stdout)");
    PipePair stdin_pipe = create_pipe_pair("CreatePipe(stdin)");
    SetHandleInformation(stdout_pipe.read_end.get(), HANDLE_FLAG_INHERIT, 0);
    SetHandleInformation(stdin_pipe.write_end.get(), HANDLE_FLAG_INHERIT, 0);

    STARTUPINFOA startup_info;
    ZeroMemory(&startup_info, sizeof(startup_info));
    startup_info.cb = sizeof(startup_info);
    startup_info.dwFlags = STARTF_USESTDHANDLES;
    startup_info.hStdInput = stdin_pipe.read_end.get();
    startup_info.hStdOutput = stdout_pipe.write_end.get();
    startup_info.hStdError = stdout_pipe.write_end.get();

    PROCESS_INFORMATION process_info;
    ZeroMemory(&process_info, sizeof(process_info));

    const std::vector<std::string> argv = platform::shell_argv(shell, login, command);
    const std::string command_line = command_line_from_argv(argv);
    std::vector<char> mutable_command_line(command_line.begin(), command_line.end());
    mutable_command_line.push_back('\0');

    const BOOL created = CreateProcessA(
        NULL,
        &mutable_command_line[0],
        NULL,
        NULL,
        TRUE,
        0,
        NULL,
        workdir.empty() ? NULL : workdir.c_str(),
        &startup_info,
        &process_info
    );

    stdin_pipe.read_end.reset();
    stdout_pipe.write_end.reset();

    if (created == 0) {
        throw std::runtime_error(last_error_message("CreateProcessA"));
    }

    std::unique_ptr<ProcessSession> session(new ProcessSession());
    session->process_handle_.reset(process_info.hProcess);
    session->stdin_write_ = std::move(stdin_pipe.write_end);
    session->stdout_read_ = std::move(stdout_pipe.read_end);

    UniqueHandle thread_handle(process_info.hThread);
    thread_handle.reset();
    return session;
}

void ProcessSession::write_stdin(const std::string& chars) {
    DWORD written = 0;
    if (WriteFile(
            stdin_write_.get(),
            chars.data(),
            static_cast<DWORD>(chars.size()),
            &written,
            NULL
        ) == 0) {
        throw std::runtime_error(last_error_message("WriteFile"));
    }
}

std::string ProcessSession::read_available(std::string* carry) {
    return read_available_console_output(stdout_read_.get(), carry);
}

std::string ProcessSession::flush_carry(std::string* carry) {
    return flush_console_output_carry(carry);
}

bool ProcessSession::has_exited(int* exit_code) {
    if (!process_handle_.valid()) {
        *exit_code = 1;
        return true;
    }
    if (WaitForSingleObject(process_handle_.get(), 0) != WAIT_OBJECT_0) {
        return false;
    }
    DWORD raw_exit_code = 0;
    GetExitCodeProcess(process_handle_.get(), &raw_exit_code);
    *exit_code = static_cast<int>(raw_exit_code);
    return true;
}

void ProcessSession::terminate() {
    if (process_handle_.valid()) {
        TerminateProcess(process_handle_.get(), 1);
        process_handle_.reset();
    }
}
#else
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

    std::unique_ptr<ProcessSession> session(new ProcessSession());
    session->pid_ = pid;
    session->stdin_write_ = std::move(stdin_pipe.write_end);
    session->stdout_read_ = std::move(stdout_pipe.read_end);
    return session;
}

void ProcessSession::write_stdin(const std::string& chars) {
    const char* data = chars.data();
    std::size_t remaining = chars.size();
    while (remaining > 0) {
        const ssize_t written = write(stdin_write_.get(), data, remaining);
        if (written < 0) {
            if (errno == EINTR) {
                continue;
            }
            throw std::runtime_error(std::string("write(stdin) failed: ") + std::strerror(errno));
        }
        if (written == 0) {
            throw std::runtime_error("write(stdin) failed");
        }
        data += written;
        remaining -= static_cast<std::size_t>(written);
    }
}

std::string ProcessSession::read_available(std::string* carry) {
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

std::string ProcessSession::flush_carry(std::string* carry) {
    return decode_utf8_output(carry, "", true);
}

bool ProcessSession::has_exited(int* exit_code) {
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

void ProcessSession::terminate() {
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
#endif

namespace {

std::shared_ptr<LiveSession> launch_live_session(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login
) {
    std::shared_ptr<LiveSession> session(new LiveSession());
    session->id = make_chunk_id();
    session->process = ProcessSession::launch(command, workdir, shell, login);
    session->started_at_ms = platform::monotonic_ms();
    return session;
}

std::string read_available(const std::shared_ptr<LiveSession>& session) {
    return session->process->read_available(&session->output_carry);
}

std::string flush_output_carry(const std::shared_ptr<LiveSession>& session) {
    return session->process->flush_carry(&session->output_carry);
}

PollResult poll_session(
    const std::shared_ptr<LiveSession>& session,
    unsigned long timeout_ms
) {
    const std::uint64_t poll_start = platform::monotonic_ms();
    std::string output;

    for (;;) {
        output += read_available(session);

        int exit_code = 0;
        if (session->process->has_exited(&exit_code)) {
            output += read_available(session);
            output += flush_output_carry(session);
            return PollResult{output, true, exit_code};
        }

        if (platform::monotonic_ms() - poll_start >= timeout_ms) {
            return PollResult{output, false, 0};
        }

        platform::sleep_ms(25);
    }
}

}  // namespace

LiveSession::LiveSession() : started_at_ms(0) {}

LiveSession::~LiveSession() {}

SessionStore::SessionStore() {}

SessionStore::~SessionStore() {
    for (std::map<std::string, std::shared_ptr<LiveSession> >::iterator it = sessions_.begin();
         it != sessions_.end();
         ++it) {
        if (it->second->process.get() != NULL) {
            it->second->process->terminate();
        }
    }
}

Json SessionStore::start_command(
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool login,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_chars,
    const YieldTimeConfig& yield_time,
    unsigned long max_open_sessions
) {
    if (sessions_.size() >= max_open_sessions) {
        throw SessionLimitError("too many open exec sessions");
    }

    {
        std::ostringstream message;
        message << "start_command cmd_preview=`" << preview_text(command, 120)
                << "` workdir=`" << workdir << "` shell=`" << shell
                << "` login=" << (login ? "true" : "false");
        log_message(LOG_INFO, "session_store", message.str());
    }
    std::shared_ptr<LiveSession> session = launch_live_session(command, workdir, shell, login);

    const unsigned long timeout_ms = resolve_yield_time_ms(
        yield_time.exec_command,
        has_yield_time_ms,
        yield_time_ms
    );
    const PollResult poll_result = poll_session(session, timeout_ms);

    if (poll_result.completed) {
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_chars
        );
        {
            std::ostringstream message;
            message << "command completed before session handoff exit_code="
                    << poll_result.exit_code
                    << " output_chars=" << poll_result.output.size();
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
    }

    sessions_[session->id] = session;
    {
        std::ostringstream message;
        message << "stored live session daemon_session_id=`" << session->id
                << "` open_sessions=" << sessions_.size();
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_response(
        session->id.c_str(),
        true,
        session->started_at_ms,
        false,
        0,
        poll_result.output,
        max_output_chars
    );
}

Json SessionStore::write_stdin(
    const std::string& daemon_session_id,
    const std::string& chars,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_chars,
    const YieldTimeConfig& yield_time
) {
    std::map<std::string, std::shared_ptr<LiveSession> >::iterator it =
        sessions_.find(daemon_session_id);
    if (it == sessions_.end()) {
        log_message(
            LOG_WARN,
            "session_store",
            "unknown daemon session `" + daemon_session_id + "`"
        );
        throw UnknownSessionError("Unknown daemon session");
    }

    {
        std::ostringstream message;
        message << "write_stdin daemon_session_id=`" << daemon_session_id
                << "` chars_len=" << chars.size();
        log_message(LOG_INFO, "session_store", message.str());
    }

    const std::shared_ptr<LiveSession>& session = it->second;
    if (!chars.empty()) {
        session->process->write_stdin(chars);
    }

    const YieldTimeOperationConfig& operation_config =
        chars.empty() ? yield_time.write_stdin_poll : yield_time.write_stdin_input;
    const unsigned long timeout_ms = resolve_yield_time_ms(
        operation_config,
        has_yield_time_ms,
        yield_time_ms
    );
    const PollResult poll_result = poll_session(session, timeout_ms);

    if (poll_result.completed) {
        Json response = build_response(
            NULL,
            false,
            session->started_at_ms,
            true,
            poll_result.exit_code,
            poll_result.output,
            max_output_chars
        );
        sessions_.erase(it);
        {
            std::ostringstream message;
            message << "session completed daemon_session_id=`" << daemon_session_id
                    << "` exit_code=" << poll_result.exit_code
                    << " open_sessions=" << sessions_.size();
            log_message(LOG_INFO, "session_store", message.str());
        }
        return response;
    }

    {
        std::ostringstream message;
        message << "session still running daemon_session_id=`" << session->id << '`';
        log_message(LOG_INFO, "session_store", message.str());
    }
    return build_response(
        session->id.c_str(),
        true,
        session->started_at_ms,
        false,
        0,
        poll_result.output,
        max_output_chars
    );
}
