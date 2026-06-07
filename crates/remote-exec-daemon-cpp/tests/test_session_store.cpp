#include "test_assert.h"
#include <atomic>
#include <cctype>
#include <cerrno>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#ifndef _WIN32
#include <signal.h>
#include <sys/stat.h>
#include <unistd.h>
#endif
#include <fstream>
#include <string>
#include <thread>
#include <vector>

#include "core/config.h"
#include "platform/platform.h"
#ifdef _WIN32
#include "platform/win32_process_tree.h"
#include "platform/win32_scoped.h"
#endif
#include "rpc/exec_http_codec.h"
#include "rpc/exec_request_utils.h"
#ifndef _WIN32
#include "exec/posix_child_reaper.h"
#endif
#include "../src/exec/session_pump_internal.h"
#include "exec/process_session.h"
#include "exec/session_store.h"
#include "test_filesystem.h"
#include "test_pty_helpers.h"
#include "test_text_file.h"

namespace fs = test_fs;

#ifdef _WIN32
const unsigned long WINDOWS_RESIZE_HELPER_SLEEP_SECONDS = 2UL;
#endif

class DrainMockProcessSession : public ProcessSession {
public:
    explicit DrainMockProcessSession(bool descendant_cleanup_supported)
        : descendant_cleanup_supported_(descendant_cleanup_supported),
          terminate_descendants_calls_(0) {}

    void write_stdin(const std::string& chars) override { (void)chars; }

    void resize_pty(unsigned short rows, unsigned short cols) override {
        (void)rows;
        (void)cols;
    }

    std::string read_output(bool block, bool* eof, std::string* carry) override {
        (void)block;
        (void)carry;
        *eof = false;
        return std::string();
    }

    std::string flush_carry(std::string* carry) override {
        carry->clear();
        return std::string();
    }

    bool has_exited(int* exit_code) override {
        *exit_code = 0;
        return true;
    }

    void terminate() override {}

    bool terminate_descendants() override {
        ++terminate_descendants_calls_;
        return descendant_cleanup_supported_;
    }

    int terminate_descendants_calls() const { return terminate_descendants_calls_; }

private:
    bool descendant_cleanup_supported_;
    int terminate_descendants_calls_;
};

static SessionOutputDrainPolicy test_drain_policy(
    unsigned long idle_ms,
    unsigned long max_ms,
    unsigned long terminate_quiet_ms
) {
    SessionOutputDrainPolicy policy;
    policy.idle_grace_ms = idle_ms;
    policy.max_grace_ms = max_ms;
    policy.terminate_quiet_ms = terminate_quiet_ms;
    return policy;
}

static void mark_mock_session_exited(LiveSession* session) {
    session->output_.exited = true;
    session->output_.exit_code = 0;
}

static void assert_explicit_drain_stop_reasons() {
    {
        LiveSession session;
        DrainMockProcessSession* process = new DrainMockProcessSession(false);
        session.process.reset(process);
        mark_mock_session_exited(&session);

        std::string output;
        BasicLockGuard lock(session.mutex_);
        const SessionOutputDrainResult result = drain_exited_session_output_locked(
            &session,
            &output,
            DEFAULT_MAX_OUTPUT_TOKENS,
            test_drain_policy(100UL, 0UL, 0UL)
        );
        TEST_ASSERT(!result.completed);
        TEST_ASSERT(result.reason == SessionOutputDrainStopReason::DescendantTerminateUnsupported);
        TEST_ASSERT(
            session.output_.last_drain_stop_reason
            == SessionOutputDrainStopReason::DescendantTerminateUnsupported
        );
        TEST_ASSERT(session.output_.descendant_cleanup_attempted);
        TEST_ASSERT(!session.output_.descendant_cleanup_supported);
        TEST_ASSERT(process->terminate_descendants_calls() == 1);

        const SessionOutputDrainResult second_result = drain_exited_session_output_locked(
            &session,
            &output,
            DEFAULT_MAX_OUTPUT_TOKENS,
            test_drain_policy(100UL, 0UL, 0UL)
        );
        TEST_ASSERT(!second_result.completed);
        TEST_ASSERT(
            second_result.reason == SessionOutputDrainStopReason::DescendantTerminateUnsupported
        );
        TEST_ASSERT(process->terminate_descendants_calls() == 1);
    }

    {
        LiveSession session;
        DrainMockProcessSession* process = new DrainMockProcessSession(true);
        session.process.reset(process);
        mark_mock_session_exited(&session);

        std::string output;
        BasicLockGuard lock(session.mutex_);
        const SessionOutputDrainResult result = drain_exited_session_output_locked(
            &session,
            &output,
            DEFAULT_MAX_OUTPUT_TOKENS,
            test_drain_policy(100UL, 0UL, 0UL)
        );
        TEST_ASSERT(!result.completed);
        TEST_ASSERT(result.reason == SessionOutputDrainStopReason::DescendantTerminateTimeout);
        TEST_ASSERT(
            session.output_.last_drain_stop_reason
            == SessionOutputDrainStopReason::DescendantTerminateTimeout
        );
        TEST_ASSERT(session.output_.descendant_cleanup_attempted);
        TEST_ASSERT(session.output_.descendant_cleanup_supported);
        TEST_ASSERT(process->terminate_descendants_calls() == 1);

        const SessionOutputDrainResult second_result = drain_exited_session_output_locked(
            &session,
            &output,
            DEFAULT_MAX_OUTPUT_TOKENS,
            test_drain_policy(100UL, 0UL, 0UL)
        );
        TEST_ASSERT(!second_result.completed);
        TEST_ASSERT(
            second_result.reason == SessionOutputDrainStopReason::DescendantTerminateTimeout
        );
        TEST_ASSERT(process->terminate_descendants_calls() == 1);
    }

    {
        LiveSession session;
        session.process.reset(new DrainMockProcessSession(false));
        mark_mock_session_exited(&session);
        session.output_.eof = true;
        session.output_.last_drain_stop_reason = SessionOutputDrainStopReason::PumpError;

        std::string output;
        BasicLockGuard lock(session.mutex_);
        const SessionOutputDrainResult result = drain_exited_session_output_locked(
            &session,
            &output,
            DEFAULT_MAX_OUTPUT_TOKENS,
            test_drain_policy(0UL, 0UL, 0UL)
        );
        TEST_ASSERT(result.completed);
        TEST_ASSERT(result.reason == SessionOutputDrainStopReason::PumpError);
        TEST_ASSERT(
            session.output_.last_drain_stop_reason == SessionOutputDrainStopReason::PumpError
        );
    }
}

#ifndef _WIN32
class ScopedEnvVar {
public:
    explicit ScopedEnvVar(const char* name) : name_(name), had_original_(false) {
        const char* original_raw = std::getenv(name_.c_str());
        had_original_ = original_raw != NULL;
        if (had_original_) {
            original_ = original_raw;
        }
    }

    ~ScopedEnvVar() {
        if (had_original_) {
            TEST_ASSERT(setenv(name_.c_str(), original_.c_str(), 1) == 0);
        } else {
            TEST_ASSERT(unsetenv(name_.c_str()) == 0);
        }
    }

    void set(const std::string& value) const {
        TEST_ASSERT(setenv(name_.c_str(), value.c_str(), 1) == 0);
    }

    void unset() const { TEST_ASSERT(unsetenv(name_.c_str()) == 0); }

private:
    std::string name_;
    std::string original_;
    bool had_original_;
};
#endif

static fs::path make_test_root() {
    const fs::path root = fs::unique_test_root("remote-exec-cpp-session-store-test");
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

static std::string normalize_output(const std::string& input) {
    std::string output;
    output.reserve(input.size());
    for (std::string::const_iterator it = input.begin(); it != input.end(); ++it) {
        if (*it == '\r') {
            continue;
        }
        if (*it == '\n') {
            while (!output.empty() && output[output.size() - 1] == ' ') {
                output.erase(output.size() - 1);
            }
        }
        output.push_back(*it);
    }
    return output;
}

#ifdef _WIN32
static std::string windows_quote_arg_for_test(const std::string& arg) {
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
    std::size_t backslashes = 0U;
    for (std::size_t i = 0; i < arg.size(); ++i) {
        const char ch = arg[i];
        if (ch == '\\') {
            ++backslashes;
            continue;
        }
        if (ch == '"') {
            quoted.append(backslashes * 2U + 1U, '\\');
            quoted.push_back('"');
            backslashes = 0U;
            continue;
        }
        quoted.append(backslashes, '\\');
        backslashes = 0U;
        quoted.push_back(ch);
    }
    quoted.append(backslashes * 2U, '\\');
    quoted.push_back('"');
    return quoted;
}

static std::string windows_stdin_echo_helper_command(const std::string& label) {
    return test_exec_pty::windows_stdin_echo_helper_command("--session-store-helper", label);
}

static std::string windows_stdin_file_helper_command(
    const std::string& file_name,
    unsigned long sleep_seconds
) {
    return test_exec_pty::windows_stdin_file_helper_command(
        "--session-store-helper",
        file_name,
        sleep_seconds
    );
}

static std::string windows_tty_flag_helper_command(const std::string& flag_file_name) {
    return test_exec_pty::windows_tty_flag_helper_command("--session-store-helper", flag_file_name);
}

static std::string windows_resize_helper_command() {
    return test_exec_pty::windows_resize_helper_command(
        "--session-store-helper",
        WINDOWS_RESIZE_HELPER_SLEEP_SECONDS
    );
}

static bool marker_count_increases(
    const fs::path& path,
    std::size_t baseline,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (fs::exists(path) && fs::read_file_bytes(path).size() > baseline) {
            return true;
        }
        platform::sleep_ms(25UL);
    }
    return fs::exists(path) && fs::read_file_bytes(path).size() > baseline;
}

static bool marker_count_stable(const fs::path& path, unsigned long timeout_ms) {
    const std::size_t before = fs::exists(path) ? fs::read_file_bytes(path).size() : 0U;
    platform::sleep_ms(timeout_ms);
    const std::size_t after = fs::exists(path) ? fs::read_file_bytes(path).size() : 0U;
    return before == after;
}

static void assert_win32_process_tree_terminates_descendants(const fs::path& root) {
    if (!win32_process_tree::process_tree_snapshot_supported()) {
        std::printf("skipping Win32 descendant process-tree termination test: process snapshots "
                    "unavailable\n");
        return;
    }

    const fs::path marker_path = root / "process-tree-marker.txt";
    fs::remove_all(marker_path);

    const std::string child_command = "for /L %i in (1,1,200) do @echo tick>>"
                                      + windows_quote_arg_for_test(marker_path.string())
                                      + " & ping -n 2 127.0.0.1>nul";
    const std::string parent_command =
        "cmd.exe /D /C start \"remote-exec-tree-test\" /B cmd.exe /D /C "
        + windows_quote_arg_for_test(child_command) + " & ping -n 31 127.0.0.1>nul";

    remote_exec_win32::NativeString native_parent_command =
        test_fs::native_from_utf8(parent_command);
    std::vector<remote_exec_win32::NativeChar> mutable_command_line(
        native_parent_command.begin(),
        native_parent_command.end()
    );
    mutable_command_line.push_back(remote_exec_win32::native_char('\0'));
    const remote_exec_win32::NativeString native_workdir = test_fs::native_from_utf8(root.string());

    remote_exec_win32::NativeStartupInfo startup_info;
    ZeroMemory(&startup_info, sizeof(startup_info));
    startup_info.cb = sizeof(startup_info);
    startup_info.dwFlags = STARTF_USESHOWWINDOW;
    startup_info.wShowWindow = SW_HIDE;

    PROCESS_INFORMATION process_info;
    ZeroMemory(&process_info, sizeof(process_info));
    TEST_ASSERT(
        remote_exec_win32::create_process_native(
            nullptr,
            &mutable_command_line[0],
            nullptr,
            nullptr,
            FALSE,
            CREATE_NO_WINDOW,
            nullptr,
            native_workdir.c_str(),
            &startup_info,
            &process_info
        )
        != 0
    );

    UniqueHandle parent_process(process_info.hProcess);
    UniqueHandle parent_thread(process_info.hThread);
    TEST_ASSERT(marker_count_increases(marker_path, 0U, 5000UL));
    const std::size_t running_count = fs::read_file_bytes(marker_path).size();
    TEST_ASSERT(marker_count_increases(marker_path, running_count, 5000UL));

    TEST_ASSERT(win32_process_tree::terminate_process_tree(process_info.dwProcessId));
    WaitForSingleObject(parent_process.get(), 5000UL);
    TEST_ASSERT(marker_count_stable(marker_path, 1500UL));
}

static bool contains_terminal_escape(const std::string& input) {
    return input.find('\x1b') != std::string::npos;
}

static bool contains_long_space_run(const std::string& input, std::size_t min_run) {
    std::size_t run = 0U;
    for (std::size_t i = 0; i < input.size(); ++i) {
        if (input[i] == ' ') {
            ++run;
            if (run >= min_run) {
                return true;
            }
        } else {
            run = 0U;
        }
    }
    return false;
}
#endif

static std::string terminal_input_line(const std::string& value) {
#ifdef _WIN32
    return value + "\r\n";
#else
    return value + "\n";
#endif
}

static Json start_test_command(
    SessionStore& store,
    const std::string& command,
    const std::string& workdir,
    const std::string& shell,
    bool tty,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time,
    unsigned long max_open_sessions
) {
    ExecStartRequestSpec request;
    request.cmd = command;
    request.workdir = workdir;
    request.shell = shell;
    request.login_requested = false;
    request.tty_requested = tty;
    request.has_yield_time_ms = true;
    request.yield_time_ms = yield_time_ms;
    request.max_output_tokens = max_output_tokens;
    const ExecSessionResult result =
        store.start_command("cpp-test", request, yield_time, max_open_sessions);
    return exec_session_result_json(result, max_output_tokens);
}

static Json write_test_stdin(
    SessionStore& store,
    const std::string& daemon_session_id,
    const std::string& chars,
    bool has_yield_time_ms,
    unsigned long yield_time_ms,
    unsigned long max_output_tokens,
    const YieldTimeConfig& yield_time,
    bool has_pty_size,
    unsigned short pty_rows,
    unsigned short pty_cols
) {
    const ExecSessionResult result = store.write_stdin(
        daemon_session_id,
        chars,
        has_yield_time_ms,
        yield_time_ms,
        max_output_tokens,
        yield_time,
        has_pty_size,
        pty_rows,
        pty_cols
    );
    return exec_session_result_json(result, max_output_tokens);
}

static std::string stable_test_shell() {
#ifdef _WIN32
    return platform::resolve_default_shell("");
#else
    return platform::resolve_default_shell("/bin/sh");
#endif
}

static YieldTimeConfig fast_yield_time_config() {
    YieldTimeConfig config;
    config.exec_command = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    config.write_stdin_poll = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    config.write_stdin_input = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    return config;
}

static bool wait_until_true(const std::atomic<bool>& value, unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (value.load()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return value.load();
}

static std::string append_running_session_output_until_pty_size(
    SessionStore& store,
    const std::string& daemon_session_id,
    const YieldTimeConfig& yield_time,
    std::string output,
    unsigned short rows,
    unsigned short cols,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (!test_exec_pty::pty_size_output_matches(output, rows, cols)
           && platform::monotonic_ms() - started < timeout_ms) {
        const Json poll = write_test_stdin(
            store,
            daemon_session_id,
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
        output += normalize_output(poll.at("output").get<std::string>());
        if (!poll.at("running").get<bool>()) {
            break;
        }
        platform::sleep_ms(10UL);
    }
    return output;
}

static Json poll_session_until_done(
    SessionStore& store,
    const std::string& daemon_session_id,
    Json response,
    const YieldTimeConfig& yield_time,
    std::string* output,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (response.at("running").get<bool>() && platform::monotonic_ms() - started < timeout_ms) {
        platform::sleep_ms(10UL);
        response = write_test_stdin(
            store,
            daemon_session_id,
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
        if (output != nullptr) {
            output->append(normalize_output(response.at("output").get<std::string>()));
        }
    }
    return response;
}

#ifdef _WIN32
static std::string append_session_output_until_done(
    SessionStore& store,
    const std::string& daemon_session_id,
    const YieldTimeConfig& yield_time,
    std::string output,
    unsigned long timeout_ms
) {
    Json poll;
    const std::uint64_t started = platform::monotonic_ms();
    do {
        poll = write_test_stdin(
            store,
            daemon_session_id,
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
        output += normalize_output(poll.at("output").get<std::string>());
        if (!poll.at("running").get<bool>()) {
            TEST_ASSERT(poll.at("exit_code").get<int>() == 0);
            return output;
        }
        platform::sleep_ms(10UL);
    } while (platform::monotonic_ms() - started < timeout_ms);

    TEST_ASSERT(false && "session did not complete before timeout");
    return output;
}

static void assert_windows_cmd_command_line_preserves_command_quotes(const std::string& shell) {
    TEST_ASSERT(
        windows_process_command_line_for_test("echo \"A & B\"", shell, false)
        == shell + " /D /C echo \"A & B\""
    );
    TEST_ASSERT(
        windows_process_command_line_for_test(
            "for /f \"tokens=1,2\" %A in (\"alpha beta\") do @echo %A:%B",
            shell,
            false
        )
        == shell + " /D /C for /f \"tokens=1,2\" %A in (\"alpha beta\") do @echo %A:%B"
    );
    TEST_ASSERT(
        windows_process_command_line_for_test("echo ok", shell, true) == shell + " /C echo ok"
    );
    TEST_ASSERT(
        windows_process_command_line_for_test("echo ok", "C:\\Program Files\\cmd.exe", false)
        == "\"C:\\Program Files\\cmd.exe\" /D /C echo ok"
    );
}

static std::string run_windows_quote_sensitive_command(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time,
    bool tty
) {
    std::string command =
        "echo \"A & B\"&for /f \"tokens=1,2\" %A in (\"alpha beta\") do @echo for:%A:%B";
    if (tty) {
        command += "&" + test_exec_pty::windows_ping_sleep_command(1UL);
    }
    const Json started = start_test_command(
        store,
        command,
        root.string(),
        shell,
        tty,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );

    std::string output = normalize_output(started.at("output").get<std::string>());
    if (started.at("running").get<bool>()) {
        output = append_session_output_until_done(
            store,
            started.at("daemon_session_id").get<std::string>(),
            yield_time,
            output,
            5000UL
        );
    } else {
        TEST_ASSERT(started.at("exit_code").get<int>() == 0);
    }
    return output;
}

static bool contains_windows_quote_sensitive_echo_output(const std::string& output) {
    return output.find("\"A & B\"\n") != std::string::npos
           || output.find("A & B\n") != std::string::npos;
}

static void assert_windows_cmd_quotes_survive_non_tty_and_tty(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
    const std::string non_tty_output =
        run_windows_quote_sensitive_command(store, root, shell, yield_time, false);
    TEST_ASSERT(non_tty_output.find("\"A & B\"\n") != std::string::npos);
    TEST_ASSERT(non_tty_output.find("for:alpha:beta\n") != std::string::npos);
    TEST_ASSERT(non_tty_output.find("\\\"") == std::string::npos);

    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

    const std::string tty_output =
        run_windows_quote_sensitive_command(store, root, shell, yield_time, true);
    if (!contains_windows_quote_sensitive_echo_output(tty_output)) {
        TEST_FAIL_MESSAGE(
            std::string("unexpected TTY quote-sensitive output: [") + tty_output + "]"
        );
    }
    TEST_ASSERT(tty_output.find("for:alpha:beta\n") != std::string::npos);
    TEST_ASSERT(tty_output.find("\\\"") == std::string::npos);
}
#endif

#ifndef _WIN32
static unsigned long warning_threshold() {
    return DEFAULT_MAX_OPEN_SESSIONS - 4UL;
}

static bool process_exists(pid_t pid) {
    if (pid <= 0) {
        return false;
    }
    if (kill(pid, 0) == 0) {
        return true;
    }
    return errno != ESRCH;
}

static pid_t read_pid_file(const fs::path& path) {
    TEST_ASSERT(fs::exists(path));
    return static_cast<pid_t>(std::strtol(fs::read_file_bytes(path).c_str(), NULL, 10));
}

static bool wait_until_process_exits(pid_t pid, unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (!process_exists(pid)) {
            return true;
        }
        platform::sleep_ms(25UL);
    }
    return !process_exists(pid);
}
#endif

static void assert_unknown_session(
    SessionStore& store,
    const std::string& daemon_session_id,
    const YieldTimeConfig& yield_time
) {
    bool rejected = false;
    try {
        (void)write_test_stdin(
            store,
            daemon_session_id,
            "",
            true,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
    } catch (const UnknownSessionError&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);
}

static void assert_completed_command_output(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    const std::string merge_command =
        "echo stdout-1 & echo stderr-1 1>&2 & echo stdout-2 & echo stderr-2 1>&2";
#else
    const std::string merge_command = "printf 'stdout-1\\n'; printf 'stderr-1\\n' >&2; "
                                      "printf 'stdout-2\\n'; printf 'stderr-2\\n' >&2";
#endif

    const Json response = start_test_command(
        store,
        merge_command,
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );

    TEST_ASSERT(response.at("daemon_session_id").is_null());
    TEST_ASSERT(!response.at("running").get<bool>());
    TEST_ASSERT(response.at("exit_code").get<int>() == 0);
    TEST_ASSERT(
        normalize_output(response.at("output").get<std::string>())
        == "stdout-1\nstderr-1\nstdout-2\nstderr-2\n"
    );
}

static void assert_token_limiting(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    const std::string print_long_command = "type long.txt";
    const std::string print_huge_command = "type huge.txt";
#else
    const std::string print_long_command = "cat long.txt";
    const std::string print_huge_command = "cat huge.txt";
#endif

    write_text_file(root / "long.txt", std::string(100, 'a'));
    const Json middle_truncated = start_test_command(
        store,
        print_long_command,
        root.string(),
        shell,
        false,
        5000UL,
        15UL,
        yield_time,
        64UL
    );
    TEST_ASSERT(middle_truncated.at("original_token_count").get<unsigned long>() == 25UL);
    TEST_ASSERT(
        normalize_output(middle_truncated.at("output").get<std::string>())
        == std::string("Total output lines: 1\n\naaaaaa") + "\xE2\x80\xA6" + "22 tokens truncated"
               + "\xE2\x80\xA6" + "aaaaaa"
    );

    write_text_file(root / "huge.txt", std::string(50000, 'x'));
    const Json omitted_limit = start_test_command(
        store,
        print_huge_command,
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(omitted_limit.at("original_token_count").get<unsigned long>() == 12500UL);
    TEST_ASSERT(
        normalize_output(omitted_limit.at("output").get<std::string>())
            .find("Total output lines: 1\n\n")
        == 0U
    );
    TEST_ASSERT(
        omitted_limit.at("output").get<std::string>().find("tokens truncated") != std::string::npos
    );

    const Json zero_limited = start_test_command(
        store,
        print_huge_command,
        root.string(),
        shell,
        false,
        5000UL,
        0UL,
        yield_time,
        64UL
    );
    TEST_ASSERT(zero_limited.at("original_token_count").get<unsigned long>() == 12500UL);
    TEST_ASSERT(zero_limited.at("output").get<std::string>().empty());
}

static void assert_posix_locale_and_late_output(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    const Json locale_response = start_test_command(
        store,
        "printf '%s %s\\n' \"$LC_ALL\" \"$LANG\"",
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(locale_response.at("exit_code").get<int>() == 0);
    TEST_ASSERT(locale_response.at("output").get<std::string>() == "C.UTF-8 C.UTF-8\n");

    const Json newline_preserved = start_test_command(
        store,
        "printf 'one two\\n'",
        root.string(),
        shell,
        false,
        5000UL,
        3UL,
        yield_time,
        64UL
    );
    TEST_ASSERT(newline_preserved.at("original_token_count").get<unsigned long>() == 2UL);
    TEST_ASSERT(newline_preserved.at("output").get<std::string>() == "one two\n");

    const Json stdin_closed_response = start_test_command(
        store,
        "if IFS= read line; then printf 'got:%s\\n' \"$line\"; "
        "else printf 'stdin:closed\\n'; fi",
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(!stdin_closed_response.at("running").get<bool>());
    TEST_ASSERT(stdin_closed_response.at("exit_code").get<int>() == 0);
    TEST_ASSERT(stdin_closed_response.at("output").get<std::string>() == "stdin:closed\n");
#endif
}

static void assert_posix_exit_drain_boundaries(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    const Json late_output = start_test_command(
        store,
        "(sleep 0.08; printf 'late tail') &",
        root.string(),
        shell,
        false,
        5000UL,
        10UL,
        yield_time,
        64UL
    );
    TEST_ASSERT(!late_output.at("running").get<bool>());
    TEST_ASSERT(late_output.at("exit_code").get<int>() == 0);
    TEST_ASSERT(late_output.at("output").get<std::string>() == "late tail");

    const Json idle_grace_output = start_test_command(
        store,
        "(sleep 0.15; printf 'idle tail') &",
        root.string(),
        shell,
        false,
        5000UL,
        10UL,
        yield_time,
        64UL
    );
    TEST_ASSERT(!idle_grace_output.at("running").get<bool>());
    TEST_ASSERT(idle_grace_output.at("exit_code").get<int>() == 0);
    TEST_ASSERT(idle_grace_output.at("output").get<std::string>() == "idle tail");

    const Json repeated_output = start_test_command(
        store,
        "(sleep 0.05; printf 'chunk1 '; sleep 0.12; printf 'chunk2 '; sleep 0.12; printf 'chunk3') "
        "&",
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(!repeated_output.at("running").get<bool>());
    TEST_ASSERT(repeated_output.at("exit_code").get<int>() == 0);
    TEST_ASSERT(repeated_output.at("output").get<std::string>() == "chunk1 chunk2 chunk3");

    const std::uint64_t max_grace_started = platform::monotonic_ms();
    const Json noisy_descendant = start_test_command(
        store,
        "(i=0; while [ $i -lt 80 ]; do printf 'tick'; i=$((i + 1)); sleep 0.05; done) &",
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    const std::uint64_t max_grace_elapsed = platform::monotonic_ms() - max_grace_started;
    TEST_ASSERT(!noisy_descendant.at("running").get<bool>());
    TEST_ASSERT(noisy_descendant.at("exit_code").get<int>() == 0);
    TEST_ASSERT(!noisy_descendant.at("output").get<std::string>().empty());
    TEST_ASSERT(max_grace_elapsed < 3500ULL && "exit drain did not enforce max grace");

    const Json stdout_held_open = start_test_command(
        store,
        "exec 3>&1; (sleep 30 >&3) & printf 'done\\n'; sleep 0.05",
        root.string(),
        shell,
        false,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(!stdout_held_open.at("running").get<bool>());
    TEST_ASSERT(stdout_held_open.at("exit_code").get<int>() == 0);
    TEST_ASSERT(stdout_held_open.at("output").get<std::string>() == "done\n");
#endif
}

static void assert_posix_exec_uses_parent_built_environment_and_path(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    const fs::path bin_dir = root / "path-bin";
    fs::create_directories(bin_dir);
    const fs::path helper = bin_dir / "env-helper";
    write_text_file(
        helper,
        "#!/bin/sh\n"
        "printf '%s|%s|%s\\n' \"$LC_ALL\" \"$LANG\" \"$TERM\"\n"
    );
    chmod(helper.c_str(), 0755);

    const char* old_path_raw = std::getenv("PATH");
    const std::string old_path = old_path_raw != NULL ? old_path_raw : "";
    ScopedEnvVar path_guard("PATH");
    ScopedEnvVar term_guard("TERM");
    const std::string new_path = bin_dir.string() + ":" + old_path;
    path_guard.set(new_path);
    term_guard.unset();

    const Json pipe_response = start_test_command(
        store,
        "env-helper",
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(pipe_response.at("exit_code").get<int>() == 0);
    const std::string pipe_output = pipe_response.at("output").get<std::string>();
    // Haiku /bin/sh initializes TERM=dumb even under env -i; LC_ALL/LANG are
    // still the daemon-provided values this test is asserting.
    TEST_ASSERT(pipe_output == "C.UTF-8|C.UTF-8|\n" || pipe_output == "C.UTF-8|C.UTF-8|dumb\n");

    if (process_session_supports_pty()) {
        const Json pty_response = start_test_command(
            store,
            "env-helper",
            root.string(),
            shell,
            true,
            5000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        TEST_ASSERT(pty_response.at("exit_code").get<int>() == 0);
        TEST_ASSERT(
            normalize_output(pty_response.at("output").get<std::string>())
            == "C.UTF-8|C.UTF-8|xterm-256color\n"
        );
    }

#endif
}

#ifndef _WIN32
#ifdef __linux__
static unsigned long zombie_children_of_current_process() {
    unsigned long zombies = 0UL;
    const fs::path proc("/proc");
    for (fs::directory_iterator it(proc), end; it != end; ++it) {
        const std::string name = it->path().filename().string();
        if (name.empty() || name.find_first_not_of("0123456789") != std::string::npos) {
            continue;
        }
        std::ifstream status((it->path() / "status").string().c_str());
        std::string line;
        bool zombie = false;
        long ppid = -1;
        while (std::getline(status, line)) {
            if (line.find("State:") == 0 && line.find("Z") != std::string::npos) {
                zombie = true;
            } else if (line.find("PPid:") == 0) {
                ppid = std::strtol(line.substr(5).c_str(), NULL, 10);
            }
        }
        if (zombie && ppid == static_cast<long>(getpid())) {
            ++zombies;
        }
    }
    return zombies;
}

static bool wait_until_zombie_delta_at_most(
    unsigned long baseline,
    unsigned long allowed_delta,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (zombie_children_of_current_process() <= baseline + allowed_delta) {
            return true;
        }
        platform::sleep_ms(25UL);
    }
    return zombie_children_of_current_process() <= baseline + allowed_delta;
}
#endif

static bool wait_until_session_exits(
    SessionStore& store,
    const std::string& session_id,
    const YieldTimeConfig& yield_time,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        const Json poll = write_test_stdin(
            store,
            session_id,
            "",
            true,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
        if (!poll.at("running").get<bool>()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return false;
}

static void assert_posix_sigchld_reaper_reaps_exited_session_children(
    const fs::path& root,
    const std::string& shell
) {
#ifdef __linux__
    const unsigned long baseline_zombies = zombie_children_of_current_process();
    SessionStore zombie_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    for (int index = 0; index < 5; ++index) {
        const Json running = start_test_command(
            zombie_store,
            "printf ready; (sleep 5 >&1) & sleep 0.2; exit 0",
            root.string(),
            shell,
            false,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            64UL
        );
        TEST_ASSERT(running.at("running").get<bool>());
    }

    TEST_ASSERT(wait_until_zombie_delta_at_most(baseline_zombies, 0UL, 2000UL));
#else
    (void)root;
    (void)shell;
#endif
}

static void assert_posix_sigchld_reaper_preserves_exit_status_during_pty_resume_race(
    const fs::path& root,
    const std::string& shell
) {
#ifdef __linux__
    if (!process_session_supports_pty()) {
        return;
    }

    const fs::path bash_path("/bin/bash");
    const std::string race_shell = fs::exists(bash_path) ? bash_path.string() : shell;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    set_posix_child_reaper_test_reap_delay_ms(200UL);
    set_process_session_test_exit_poll_delay_ms(50UL);

    for (int attempt = 0; attempt < 10; ++attempt) {
        const std::string ready_file_name = "pty-race-ready-" + std::to_string(attempt) + ".txt";
        const fs::path ready_path = root / ready_file_name;
        fs::remove_all(ready_path);

        SessionStore race_store;
        Json waiting = start_test_command(
            race_store,
            "printf 'ready\\n'; printf 'ready\\n' > " + ready_file_name
                + "; IFS= read line; printf 'echo:%s\\n' \"$line\"",
            root.string(),
            race_shell,
            true,
            50UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            64UL
        );
        TEST_ASSERT(waiting.at("running").get<bool>());
        std::string waiting_id = waiting.at("daemon_session_id").get<std::string>();
        std::string waiting_output = waiting.at("output").get<std::string>();
        TEST_ASSERT(test_exec_pty::wait_until_file_contains(ready_path, "ready", 5000UL));

        const Json resumed = write_test_stdin(
            race_store,
            waiting_id,
            "ping\n",
            true,
            1000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            false,
            0U,
            0U
        );
        const std::string resumed_output = waiting_output + resumed.at("output").get<std::string>();
        TEST_ASSERT(!resumed.at("running").get<bool>());
        TEST_ASSERT(resumed.at("exit_code").get<int>() == 0);
        TEST_ASSERT(resumed_output.find("ready") != std::string::npos);
        TEST_ASSERT(resumed_output.find("echo:ping") != std::string::npos);
    }

    set_process_session_test_exit_poll_delay_ms(0UL);
    set_posix_child_reaper_test_reap_delay_ms(0UL);
#else
    (void)root;
    (void)shell;
#endif
}
#endif

#ifndef _WIN32
static void assert_non_tty_stdin_closed_rejected(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
    const Json non_tty_running = start_test_command(
        store,
        "printf ready; sleep 5",
        root.string(),
        shell,
        false,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(non_tty_running.at("running").get<bool>());
    TEST_ASSERT(non_tty_running.at("output").get<std::string>() == "ready");

    bool stdin_closed_rejected = false;
    try {
        (void)write_test_stdin(
            store,
            non_tty_running.at("daemon_session_id").get<std::string>(),
            "hello\n",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
    } catch (const StdinClosedError& ex) {
        stdin_closed_rejected = std::string(ex.what()).find("stdin is closed") != std::string::npos;
    }
    TEST_ASSERT(stdin_closed_rejected);
}
#endif

static void assert_terminal_write_removes_completed_session(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell
) {
    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

    const YieldTimeConfig fast_yield = fast_yield_time_config();
#ifdef _WIN32
    const std::string command = windows_stdin_echo_helper_command("final");
#else
    const std::string command = "printf 'ready\\n'; IFS= read line; printf 'final:%s\\n' \"$line\"";
#endif
    const Json waiting = start_test_command(
        store,
        command,
        root.string(),
        shell,
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        64UL
    );
    TEST_ASSERT(waiting.at("running").get<bool>());
    const std::string session_id = waiting.at("daemon_session_id").get<std::string>();

    Json completed = write_test_stdin(
        store,
        session_id,
        terminal_input_line("done"),
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        false,
        0U,
        0U
    );
    std::string output = normalize_output(completed.at("output").get<std::string>());
    completed = poll_session_until_done(store, session_id, completed, fast_yield, &output, 5000UL);
    TEST_ASSERT(!completed.at("running").get<bool>());
    TEST_ASSERT(completed.at("daemon_session_id").is_null());
    TEST_ASSERT(completed.at("exit_code").get<int>() == 0);
    TEST_ASSERT(completed.at("original_token_count").get<unsigned long>() >= 2UL);
    TEST_ASSERT(output.find("final:done") != std::string::npos);
    assert_unknown_session(store, session_id, fast_yield);
}

static void assert_tty_resume_round_trip(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell
) {
    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

    const YieldTimeConfig fast_yield = fast_yield_time_config();
#ifdef _WIN32
    const std::string command = windows_stdin_echo_helper_command("echo");
#else
    const std::string command = "printf 'ready\\n'; IFS= read line; printf 'echo:%s\\n' \"$line\"";
#endif
    const Json waiting = start_test_command(
        store,
        command,
        root.string(),
        shell,
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        64UL
    );
    TEST_ASSERT(waiting.at("running").get<bool>());

    const std::string waiting_id = waiting.at("daemon_session_id").get<std::string>();
    Json resumed = write_test_stdin(
        store,
        waiting_id,
        terminal_input_line("ping"),
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        false,
        0U,
        0U
    );
    std::string resume_output = normalize_output(waiting.at("output").get<std::string>())
                                + normalize_output(resumed.at("output").get<std::string>());
    const std::uint64_t resume_started = platform::monotonic_ms();
    while (resumed.at("running").get<bool>() && platform::monotonic_ms() - resume_started < 5000UL
    ) {
        resumed = write_test_stdin(
            store,
            waiting_id,
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            false,
            0U,
            0U
        );
        resume_output += normalize_output(resumed.at("output").get<std::string>());
    }
    TEST_ASSERT(!resumed.at("running").get<bool>());
    TEST_ASSERT(resumed.at("exit_code").get<int>() == 0);
    TEST_ASSERT(resume_output.find("echo:ping\n") != std::string::npos);
}

static void assert_unrelated_sessions_do_not_block_each_other(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

#ifdef _WIN32
    const std::string slow_command = "echo slow&" + test_exec_pty::windows_ping_sleep_command(30UL);
    const std::string fast_command =
        windows_stdin_file_helper_command("fast-session-input.txt", 30UL);
#else
    const std::string slow_command = "printf slow; sleep 30";
    const std::string fast_command =
        "IFS= read line; printf '%s' \"$line\" > fast-session-input.txt; sleep 30";
#endif
    const Json slow_running = start_test_command(
        store,
        slow_command,
        root.string(),
        shell,
        true,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(slow_running.at("running").get<bool>());

    const fs::path fast_input_path = root / "fast-session-input.txt";
    fs::remove_all(fast_input_path);
    const Json fast_running = start_test_command(
        store,
        fast_command,
        root.string(),
        shell,
        true,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(fast_running.at("running").get<bool>());

    Json slow_poll;
    std::atomic<bool> slow_thread_started(false);
    std::thread slow_thread([&]() {
        slow_thread_started.store(true);
        slow_poll = write_test_stdin(
            store,
            slow_running.at("daemon_session_id").get<std::string>(),
            "",
            true,
            5000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
    });

    TEST_ASSERT(wait_until_true(slow_thread_started, 1000UL));
    const std::uint64_t fast_started_at = platform::monotonic_ms();
    const Json fast_completed = write_test_stdin(
        store,
        fast_running.at("daemon_session_id").get<std::string>(),
        terminal_input_line("ping"),
        true,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        false,
        0U,
        0U
    );
    const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
    TEST_ASSERT(fast_elapsed_ms < 2000UL && "fast session waited behind unrelated session");
    (void)fast_completed;
    TEST_ASSERT(test_exec_pty::wait_until_file_contains(fast_input_path, "ping", 2000UL));
    slow_thread.join();
    TEST_ASSERT(slow_poll.at("running").get<bool>());
}

static void assert_tty_detection_and_input_round_trip(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

    const fs::path tty_flag_path = root / "tty-detected.txt";
    fs::remove_all(tty_flag_path);
#ifdef _WIN32
    const std::string command = windows_tty_flag_helper_command("tty-detected.txt");
#else
    const std::string command = "if test -t 0; then printf yes > tty-detected.txt; else printf no "
                                "> tty-detected.txt; fi; IFS= read line; "
                                "printf 'input:%s\\n' \"$line\"";
#endif
    const Json tty_running = start_test_command(
        store,
        command,
        root.string(),
        shell,
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(tty_running.at("running").get<bool>());
    std::string tty_output = normalize_output(tty_running.at("output").get<std::string>());

    Json tty_completed = write_test_stdin(
        store,
        tty_running.at("daemon_session_id").get<std::string>(),
        terminal_input_line("hello"),
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        false,
        0U,
        0U
    );
    tty_output += normalize_output(tty_completed.at("output").get<std::string>());
    const std::uint64_t tty_started = platform::monotonic_ms();
    while (tty_completed.at("running").get<bool>()
           && platform::monotonic_ms() - tty_started < 5000UL) {
        tty_completed = write_test_stdin(
            store,
            tty_running.at("daemon_session_id").get<std::string>(),
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
        tty_output += normalize_output(tty_completed.at("output").get<std::string>());
    }
    TEST_ASSERT(!tty_completed.at("running").get<bool>());
    TEST_ASSERT(tty_completed.at("exit_code").get<int>() == 0);
    TEST_ASSERT(test_exec_pty::wait_until_file_contains(tty_flag_path, "yes", 2000UL));
    TEST_ASSERT(tty_output.find("hello\n") != std::string::npos);
    TEST_ASSERT(tty_output.find("input:hello\n") != std::string::npos);
#ifdef _WIN32
    TEST_ASSERT(!contains_terminal_escape(tty_output));
#endif
}

static void assert_tty_resize_round_trip(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell
) {
    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

    const YieldTimeConfig fast_yield = fast_yield_time_config();
#ifdef _WIN32
    const std::string command = windows_resize_helper_command();
#else
    const std::string command = "printf ready; IFS= read line; stty -a; sleep 30";
#endif
    const Json resize_running = start_test_command(
        store,
        command,
        root.string(),
        shell,
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        64UL
    );
    TEST_ASSERT(resize_running.at("running").get<bool>());
    const Json resized = write_test_stdin(
        store,
        resize_running.at("daemon_session_id").get<std::string>(),
        terminal_input_line(""),
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        true,
        33U,
        101U
    );
    TEST_ASSERT(resized.at("running").get<bool>());
#ifdef _WIN32
    return;
#endif
    std::string resize_output = normalize_output(resized.at("output").get<std::string>());
    if (!test_exec_pty::pty_size_output_matches(resize_output, 33U, 101U)) {
        resize_output = append_running_session_output_until_pty_size(
            store,
            resize_running.at("daemon_session_id").get<std::string>(),
            fast_yield,
            resize_output,
            33U,
            101U,
            2000UL
        );
    }
    TEST_ASSERT(test_exec_pty::pty_size_output_matches(resize_output, 33U, 101U));
}

static void assert_non_tty_resize_rejected(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

#ifdef _WIN32
    const std::string command = "echo ready&ping -n 6 127.0.0.1>nul";
#else
    const std::string command = "printf ready; sleep 5";
#endif
    const Json non_tty_running = start_test_command(
        store,
        command,
        root.string(),
        shell,
        false,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(non_tty_running.at("running").get<bool>());

    bool non_tty_resize_rejected = false;
    try {
        (void)write_test_stdin(
            store,
            non_tty_running.at("daemon_session_id").get<std::string>(),
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            true,
            33U,
            101U
        );
    } catch (const ProcessPtyResizeUnsupportedError& ex) {
        non_tty_resize_rejected =
            std::string(ex.what()).find("requires a tty session") != std::string::npos;
    }
    TEST_ASSERT(non_tty_resize_rejected);
}

#ifndef _WIN32
static void assert_session_store_destruction_terminates_process_group(
    const fs::path& root,
    const std::string& shell
) {
    const fs::path parent_pid_path = root / "destructor-parent.pid";
    const fs::path child_pid_path = root / "destructor-child.pid";
    fs::remove_all(parent_pid_path);
    fs::remove_all(child_pid_path);

    {
        SessionStore scoped_store;
        const YieldTimeConfig fast_yield = fast_yield_time_config();
        const Json running = start_test_command(
            scoped_store,
            "printf '%s' $$ > destructor-parent.pid; "
            "(sleep 30) & printf '%s' $! > destructor-child.pid; "
            "printf ready; sleep 30",
            root.string(),
            shell,
            false,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            64UL
        );
        TEST_ASSERT(running.at("running").get<bool>());
        TEST_ASSERT(test_exec_pty::wait_until_file_contains(parent_pid_path, "", 2000UL));
        TEST_ASSERT(test_exec_pty::wait_until_file_contains(child_pid_path, "", 2000UL));
    }

    const pid_t parent_pid = read_pid_file(parent_pid_path);
    const pid_t child_pid = read_pid_file(child_pid_path);
    TEST_ASSERT(wait_until_process_exits(parent_pid, 2000UL));
    TEST_ASSERT(wait_until_process_exits(child_pid, 2000UL));
}

static void assert_session_limit_prunes_oldest_running(
    const fs::path& root,
    const std::string& shell
) {
    SessionStore limit_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json first_running = start_test_command(
        limit_store,
        "printf 'first'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    const Json second_running = start_test_command(
        limit_store,
        "printf 'second'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    const Json third_running = start_test_command(
        limit_store,
        "printf 'third'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    TEST_ASSERT(first_running.at("running").get<bool>());
    TEST_ASSERT(second_running.at("running").get<bool>());
    TEST_ASSERT(third_running.at("running").get<bool>());
    assert_unknown_session(
        limit_store,
        first_running.at("daemon_session_id").get<std::string>(),
        fast_yield
    );
    TEST_ASSERT(write_test_stdin(
                    limit_store,
                    second_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield,
                    false,
                    0U,
                    0U
    )
                    .at("running")
                    .get<bool>());
    TEST_ASSERT(write_test_stdin(
                    limit_store,
                    third_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield,
                    false,
                    0U,
                    0U
    )
                    .at("running")
                    .get<bool>());
}

static void assert_recent_session_survives_limit_prune(
    const fs::path& root,
    const std::string& shell
) {
    SessionStore recency_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json first_running = start_test_command(
        recency_store,
        "printf 'first'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    const Json second_running = start_test_command(
        recency_store,
        "printf 'second'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    const Json first_touch = write_test_stdin(
        recency_store,
        first_running.at("daemon_session_id").get<std::string>(),
        "",
        true,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        false,
        0U,
        0U
    );
    TEST_ASSERT(first_touch.at("running").get<bool>());

    const Json third_running = start_test_command(
        recency_store,
        "printf 'third'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    TEST_ASSERT(third_running.at("running").get<bool>());
    assert_unknown_session(
        recency_store,
        second_running.at("daemon_session_id").get<std::string>(),
        fast_yield
    );
    TEST_ASSERT(write_test_stdin(
                    recency_store,
                    first_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield,
                    false,
                    0U,
                    0U
    )
                    .at("running")
                    .get<bool>());
}

static void assert_exited_session_is_pruned_before_live_session(
    const fs::path& root,
    const std::string& shell
) {
    SessionStore exited_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json exited_running = start_test_command(
        exited_store,
        "sleep 0.05",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    const Json live_running = start_test_command(
        exited_store,
        "printf 'live'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    TEST_ASSERT(exited_running.at("running").get<bool>());
    TEST_ASSERT(live_running.at("running").get<bool>());
    TEST_ASSERT(wait_until_session_exits(
        exited_store,
        exited_running.at("daemon_session_id").get<std::string>(),
        fast_yield,
        2000UL
    ));

    const Json replacement_running = start_test_command(
        exited_store,
        "printf 'replacement'; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        2UL
    );
    TEST_ASSERT(replacement_running.at("running").get<bool>());
    assert_unknown_session(
        exited_store,
        exited_running.at("daemon_session_id").get<std::string>(),
        fast_yield
    );
    TEST_ASSERT(write_test_stdin(
                    exited_store,
                    live_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield,
                    false,
                    0U,
                    0U
    )
                    .at("running")
                    .get<bool>());
}

static void assert_recent_session_is_protected_from_prune(
    const fs::path& root,
    const std::string& shell
) {
    SessionStore protected_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    std::vector<std::string> daemon_session_ids;
    for (int index = 0; index < 10; ++index) {
        const Json running = start_test_command(
            protected_store,
            "printf ready; sleep 30",
            root.string(),
            shell,
            false,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            10UL
        );
        TEST_ASSERT(running.at("running").get<bool>());
        daemon_session_ids.push_back(running.at("daemon_session_id").get<std::string>());
    }

    TEST_ASSERT(write_test_stdin(
                    protected_store,
                    daemon_session_ids[0],
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield,
                    false,
                    0U,
                    0U
    )
                    .at("running")
                    .get<bool>());

    const Json protected_replacement = start_test_command(
        protected_store,
        "printf extra; sleep 30",
        root.string(),
        shell,
        false,
        1UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        fast_yield,
        10UL
    );
    TEST_ASSERT(protected_replacement.at("running").get<bool>());
    assert_unknown_session(protected_store, daemon_session_ids[1], fast_yield);
    TEST_ASSERT(write_test_stdin(
                    protected_store,
                    daemon_session_ids[0],
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield,
                    false,
                    0U,
                    0U
    )
                    .at("running")
                    .get<bool>());
}
#endif

static void assert_stdin_and_tty_behavior(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifndef _WIN32
    assert_non_tty_stdin_closed_rejected(store, root, shell, yield_time);
#else
    const Json xp_running = start_test_command(
        store,
        windows_stdin_echo_helper_command("got"),
        root.string(),
        shell,
        false,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(xp_running.at("running").get<bool>());
    const std::string xp_initial = normalize_output(xp_running.at("output").get<std::string>());

    const std::string xp_session_id = xp_running.at("daemon_session_id").get<std::string>();
    Json xp_completed = write_test_stdin(
        store,
        xp_session_id,
        "hello\r\n",
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        false,
        0U,
        0U
    );
    std::string xp_output =
        xp_initial + normalize_output(xp_completed.at("output").get<std::string>());
    xp_completed =
        poll_session_until_done(store, xp_session_id, xp_completed, yield_time, &xp_output, 5000UL);
    TEST_ASSERT(!xp_completed.at("running").get<bool>());
    TEST_ASSERT(xp_completed.at("exit_code").get<int>() == 0);
    TEST_ASSERT(xp_output.find("ready\n") != std::string::npos);
    TEST_ASSERT(xp_output.find("got:hello\n") != std::string::npos);

    if (test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        return;
    }

    const Json cmd_running = start_test_command(
        store,
        "cmd.exe",
        root.string(),
        shell,
        true,
        1000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    TEST_ASSERT(cmd_running.at("running").get<bool>());
    const std::string cmd_session_id = cmd_running.at("daemon_session_id").get<std::string>();
    std::string cmd_output = normalize_output(cmd_running.at("output").get<std::string>());
    Json cmd_response = write_test_stdin(
        store,
        cmd_session_id,
        "echo hello\r\n",
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        false,
        0U,
        0U
    );
    cmd_output += normalize_output(cmd_response.at("output").get<std::string>());
    cmd_response = write_test_stdin(
        store,
        cmd_session_id,
        "exit\r\n",
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        false,
        0U,
        0U
    );
    cmd_output += normalize_output(cmd_response.at("output").get<std::string>());
    cmd_response = poll_session_until_done(
        store,
        cmd_session_id,
        cmd_response,
        yield_time,
        &cmd_output,
        5000UL
    );
    TEST_ASSERT(!cmd_response.at("running").get<bool>());
    TEST_ASSERT(cmd_response.at("exit_code").get<int>() == 0);
    TEST_ASSERT(cmd_output.find("echo hello") != std::string::npos);
    TEST_ASSERT(cmd_output.find("hello\n") != std::string::npos);
    TEST_ASSERT(cmd_output.find("C:\\") != std::string::npos);
    TEST_ASSERT(!contains_terminal_escape(cmd_output));
    TEST_ASSERT(!contains_long_space_run(cmd_output, 32U));
#endif

    assert_terminal_write_removes_completed_session(store, root, shell);
    assert_tty_resume_round_trip(store, root, shell);
    assert_unrelated_sessions_do_not_block_each_other(store, root, shell, yield_time);
    assert_tty_detection_and_input_round_trip(store, root, shell, yield_time);
    assert_tty_resize_round_trip(store, root, shell);
    assert_non_tty_resize_rejected(store, root, shell, yield_time);
}

static void assert_pruning_and_recency_behavior(const fs::path& root, const std::string& shell) {
#ifdef _WIN32
    (void)root;
    (void)shell;
#else
    assert_session_limit_prunes_oldest_running(root, shell);
    assert_recent_session_survives_limit_prune(root, shell);
    assert_exited_session_is_pruned_before_live_session(root, shell);
    assert_recent_session_is_protected_from_prune(root, shell);
    assert_session_store_destruction_terminates_process_group(root, shell);
#endif
}

static void assert_threshold_warnings_and_unknown_sessions(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    {
        SessionStore warning_store;
        const YieldTimeConfig fast_yield = fast_yield_time_config();
        const unsigned long threshold = warning_threshold();
        Json threshold_response;
        for (unsigned long index = 0; index < threshold; ++index) {
            const Json running = start_test_command(
                warning_store,
                "printf ready; sleep 30",
                root.string(),
                shell,
                false,
                1UL,
                DEFAULT_MAX_OUTPUT_TOKENS,
                fast_yield,
                DEFAULT_MAX_OPEN_SESSIONS
            );
            TEST_ASSERT(running.at("running").get<bool>());
            if (index + 1UL < threshold) {
                TEST_ASSERT(running.at("warnings").empty());
            } else {
                threshold_response = running;
            }
        }
        TEST_ASSERT(threshold_response.at("warnings").size() == 1U);
        TEST_ASSERT(
            threshold_response.at("warnings")[0].at("code").get<std::string>()
            == "exec_session_limit_approaching"
        );
        TEST_ASSERT(
            threshold_response.at("warnings")[0].at("message").get<std::string>()
            == "Target `cpp-test` now has " + std::to_string(threshold) + " open exec sessions."
        );
    }

    bool unknown_session_rejected = false;
    try {
        (void)write_test_stdin(
            store,
            "missing-session",
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
    } catch (const UnknownSessionError&) {
        unknown_session_rejected = true;
    }
    TEST_ASSERT(unknown_session_rejected);
#endif
}

static void assert_threshold_warnings_follow_configured_limit(
    const fs::path& root,
    const std::string& shell
) {
#ifdef _WIN32
    (void)root;
    (void)shell;
#else
    SessionStore warning_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const unsigned long max_open_sessions = 6UL;
    const unsigned long threshold = max_open_sessions - 4UL;
    Json threshold_response;
    for (unsigned long index = 0; index < threshold; ++index) {
        const Json running = start_test_command(
            warning_store,
            "printf ready; sleep 30",
            root.string(),
            shell,
            false,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            max_open_sessions
        );
        TEST_ASSERT(running.at("running").get<bool>());
        if (index + 1UL < threshold) {
            TEST_ASSERT(running.at("warnings").empty());
        } else {
            threshold_response = running;
        }
    }
    TEST_ASSERT(threshold_response.at("warnings").size() == 1U);
    TEST_ASSERT(
        threshold_response.at("warnings")[0].at("code").get<std::string>()
        == "exec_session_limit_approaching"
    );
    TEST_ASSERT(
        threshold_response.at("warnings")[0].at("message").get<std::string>()
        == "Target `cpp-test` now has " + std::to_string(threshold) + " open exec sessions."
    );
#endif
}

int main(int argc, char** argv) {
#ifdef _WIN32
    if (argc >= 2 && std::strcmp(argv[1], "--session-store-helper") == 0) {
        return test_exec_pty::run_windows_stdin_helper(argc, argv, 2);
    }
#else
    (void)argc;
    (void)argv;
#endif
#ifndef _WIN32
    install_posix_child_reaper();
#endif
    const fs::path root = make_test_root();
    SessionStore store;
    const YieldTimeConfig yield_time = YieldTimeConfig();
    const std::string shell = stable_test_shell();

#ifdef _WIN32
    assert_windows_cmd_command_line_preserves_command_quotes(shell);
    assert_win32_process_tree_terminates_descendants(root);
#endif
    assert_explicit_drain_stop_reasons();
    assert_completed_command_output(store, root, shell, yield_time);
    assert_token_limiting(store, root, shell, yield_time);
#ifdef _WIN32
    assert_windows_cmd_quotes_survive_non_tty_and_tty(store, root, shell, yield_time);
#endif
    assert_posix_locale_and_late_output(store, root, shell, yield_time);
    assert_posix_exit_drain_boundaries(store, root, shell, yield_time);
    assert_posix_exec_uses_parent_built_environment_and_path(store, root, shell, yield_time);
#ifndef _WIN32
    assert_posix_sigchld_reaper_reaps_exited_session_children(root, shell);
    assert_posix_sigchld_reaper_preserves_exit_status_during_pty_resume_race(root, shell);
#endif
    assert_stdin_and_tty_behavior(store, root, shell, yield_time);
    assert_pruning_and_recency_behavior(root, shell);
    assert_threshold_warnings_and_unknown_sessions(store, root, shell, yield_time);
    assert_threshold_warnings_follow_configured_limit(root, shell);

    return 0;
}
