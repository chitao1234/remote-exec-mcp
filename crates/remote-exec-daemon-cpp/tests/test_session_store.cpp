#include <atomic>
#include <cassert>
#include <cstdint>
#ifndef _WIN32
#include <cstdlib>
#include <sys/stat.h>
#include <unistd.h>
#endif
#include <fstream>
#include <string>
#include <thread>
#include <vector>

#include "config.h"
#include "platform.h"
#ifndef _WIN32
#include "posix_child_reaper.h"
#endif
#include "process_session.h"
#include "session_store.h"
#include "test_filesystem.h"
#include "test_text_file.h"

namespace fs = test_fs;

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
            assert(setenv(name_.c_str(), original_.c_str(), 1) == 0);
        } else {
            assert(unsetenv(name_.c_str()) == 0);
        }
    }

    void set(const std::string& value) const {
        assert(setenv(name_.c_str(), value.c_str(), 1) == 0);
    }

    void unset() const {
        assert(unsetenv(name_.c_str()) == 0);
    }

  private:
    std::string name_;
    std::string original_;
    bool had_original_;
};
#endif

static fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-session-store-test";
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

static Json start_test_command(SessionStore& store,
                               const std::string& command,
                               const std::string& workdir,
                               const std::string& shell,
                               bool tty,
                               unsigned long yield_time_ms,
                               unsigned long max_output_tokens,
                               const YieldTimeConfig& yield_time,
                               unsigned long max_open_sessions) {
    return store.start_command("cpp-test",
                               command,
                               workdir,
                               shell,
                               false,
                               tty,
                               true,
                               yield_time_ms,
                               max_output_tokens,
                               yield_time,
                               max_open_sessions);
}

static YieldTimeConfig fast_yield_time_config() {
    YieldTimeConfig config;
    config.exec_command = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    config.write_stdin_poll = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    config.write_stdin_input = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    return config;
}

static unsigned long warning_threshold() {
    return DEFAULT_MAX_OPEN_SESSIONS - 4UL;
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

static void
assert_unknown_session(SessionStore& store, const std::string& daemon_session_id, const YieldTimeConfig& yield_time) {
    bool rejected = false;
    try {
        (void)store.write_stdin(daemon_session_id, "", true, 1UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, false, 0U, 0U);
    } catch (const UnknownSessionError&) {
        rejected = true;
    }
    assert(rejected);
}

static void assert_completed_command_output(SessionStore& store,
                                            const fs::path& root,
                                            const std::string& shell,
                                            const YieldTimeConfig& yield_time) {
#ifdef _WIN32
    const std::string merge_command = "echo stdout-1 & echo stderr-1 1>&2 & echo stdout-2 & echo stderr-2 1>&2";
#else
    const std::string merge_command = "printf 'stdout-1\\n'; printf 'stderr-1\\n' >&2; "
                                      "printf 'stdout-2\\n'; printf 'stderr-2\\n' >&2";
#endif

    const Json response = start_test_command(
        store, merge_command, root.string(), shell, false, 5000UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, 64UL);

    assert(response.at("daemon_session_id").is_null());
    assert(!response.at("running").get<bool>());
    assert(response.at("exit_code").get<int>() == 0);
    assert(normalize_output(response.at("output").get<std::string>()) == "stdout-1\nstderr-1\nstdout-2\nstderr-2\n");
}

static void assert_token_limiting(SessionStore& store,
                                  const fs::path& root,
                                  const std::string& shell,
                                  const YieldTimeConfig& yield_time) {
#ifdef _WIN32
    const std::string print_long_command = "type long.txt";
    const std::string print_huge_command = "type huge.txt";
#else
    const std::string print_long_command = "cat long.txt";
    const std::string print_huge_command = "cat huge.txt";
#endif

    write_text_file(root / "long.txt", std::string(100, 'a'));
    const Json middle_truncated =
        start_test_command(store, print_long_command, root.string(), shell, false, 5000UL, 15UL, yield_time, 64UL);
    assert(middle_truncated.at("original_token_count").get<unsigned long>() == 25UL);
    assert(normalize_output(middle_truncated.at("output").get<std::string>()) ==
           std::string("Total output lines: 1\n\naaaaaa") + "\xE2\x80\xA6" + "22 tokens truncated" + "\xE2\x80\xA6" +
               "aaaaaa");

    write_text_file(root / "huge.txt", std::string(50000, 'x'));
    const Json omitted_limit = start_test_command(
        store, print_huge_command, root.string(), shell, false, 5000UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, 64UL);
    assert(omitted_limit.at("original_token_count").get<unsigned long>() == 12500UL);
    assert(normalize_output(omitted_limit.at("output").get<std::string>()).find("Total output lines: 1\n\n") == 0U);
    assert(omitted_limit.at("output").get<std::string>().find("tokens truncated") != std::string::npos);

    const Json zero_limited =
        start_test_command(store, print_huge_command, root.string(), shell, false, 5000UL, 0UL, yield_time, 64UL);
    assert(zero_limited.at("original_token_count").get<unsigned long>() == 12500UL);
    assert(zero_limited.at("output").get<std::string>().empty());
}

static void assert_posix_locale_and_late_output(SessionStore& store,
                                                const fs::path& root,
                                                const std::string& shell,
                                                const YieldTimeConfig& yield_time) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    const Json locale_response = start_test_command(store,
                                                    "printf '%s %s\\n' \"$LC_ALL\" \"$LANG\"",
                                                    root.string(),
                                                    shell,
                                                    false,
                                                    5000UL,
                                                    DEFAULT_MAX_OUTPUT_TOKENS,
                                                    yield_time,
                                                    64UL);
    assert(locale_response.at("exit_code").get<int>() == 0);
    assert(locale_response.at("output").get<std::string>() == "C.UTF-8 C.UTF-8\n");

    const Json late_output = start_test_command(
        store, "(sleep 0.08; printf 'late tail') &", root.string(), shell, false, 5000UL, 10UL, yield_time, 64UL);
    assert(!late_output.at("running").get<bool>());
    assert(late_output.at("exit_code").get<int>() == 0);
    assert(late_output.at("output").get<std::string>() == "late tail");

    const Json stdout_held_open = start_test_command(store,
                                                     "exec 3>&1; (sleep 30 >&3) & printf 'done\\n'; sleep 0.05",
                                                     root.string(),
                                                     shell,
                                                     false,
                                                     250UL,
                                                     DEFAULT_MAX_OUTPUT_TOKENS,
                                                     yield_time,
                                                     64UL);
    assert(!stdout_held_open.at("running").get<bool>());
    assert(stdout_held_open.at("exit_code").get<int>() == 0);
    assert(stdout_held_open.at("output").get<std::string>() == "done\n");

    const Json newline_preserved =
        start_test_command(store, "printf 'one two\\n'", root.string(), shell, false, 5000UL, 3UL, yield_time, 64UL);
    assert(newline_preserved.at("original_token_count").get<unsigned long>() == 2UL);
    assert(newline_preserved.at("output").get<std::string>() == "one two\n");

    const Json stdin_closed_response = start_test_command(store,
                                                          "if IFS= read line; then printf 'got:%s\\n' \"$line\"; "
                                                          "else printf 'stdin:closed\\n'; fi",
                                                          root.string(),
                                                          shell,
                                                          false,
                                                          5000UL,
                                                          DEFAULT_MAX_OUTPUT_TOKENS,
                                                          yield_time,
                                                          64UL);
    assert(!stdin_closed_response.at("running").get<bool>());
    assert(stdin_closed_response.at("exit_code").get<int>() == 0);
    assert(stdin_closed_response.at("output").get<std::string>() == "stdin:closed\n");
#endif
}

static void assert_posix_exec_uses_parent_built_environment_and_path(SessionStore& store,
                                                                     const fs::path& root,
                                                                     const std::string& shell,
                                                                     const YieldTimeConfig& yield_time) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    const fs::path bin_dir = root / "path-bin";
    fs::create_directories(bin_dir);
    const fs::path helper = bin_dir / "env-helper";
    write_text_file(helper,
                    "#!/bin/sh\n"
                    "printf '%s|%s|%s\\n' \"$LC_ALL\" \"$LANG\" \"$TERM\"\n");
    chmod(helper.c_str(), 0755);

    const char* old_path_raw = std::getenv("PATH");
    const std::string old_path = old_path_raw != NULL ? old_path_raw : "";
    ScopedEnvVar path_guard("PATH");
    ScopedEnvVar term_guard("TERM");
    const std::string new_path = bin_dir.string() + ":" + old_path;
    path_guard.set(new_path);
    term_guard.unset();

    const Json pipe_response = start_test_command(
        store, "env-helper", root.string(), shell, false, 5000UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, 64UL);
    assert(pipe_response.at("exit_code").get<int>() == 0);
    assert(pipe_response.at("output").get<std::string>() == "C.UTF-8|C.UTF-8|\n");

    if (process_session_supports_pty()) {
        const Json pty_response = start_test_command(
            store, "env-helper", root.string(), shell, true, 5000UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, 64UL);
        assert(pty_response.at("exit_code").get<int>() == 0);
        assert(normalize_output(pty_response.at("output").get<std::string>()) == "C.UTF-8|C.UTF-8|xterm-256color\n");
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

static bool
wait_until_zombie_delta_at_most(unsigned long baseline, unsigned long allowed_delta, unsigned long timeout_ms) {
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

static bool wait_until_session_exits(SessionStore& store,
                                     const std::string& session_id,
                                     const YieldTimeConfig& yield_time,
                                     unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        const Json poll =
            store.write_stdin(session_id, "", true, 1UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, false, 0U, 0U);
        if (!poll.at("running").get<bool>()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return false;
}

static void assert_posix_sigchld_reaper_reaps_exited_session_children(const fs::path& root, const std::string& shell) {
#ifdef __linux__
    const unsigned long baseline_zombies = zombie_children_of_current_process();
    SessionStore zombie_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    for (int index = 0; index < 5; ++index) {
        const Json running = start_test_command(zombie_store,
                                                "printf ready; (sleep 5 >&1) & sleep 0.2; exit 0",
                                                root.string(),
                                                shell,
                                                false,
                                                1UL,
                                                DEFAULT_MAX_OUTPUT_TOKENS,
                                                fast_yield,
                                                64UL);
        assert(running.at("running").get<bool>());
    }

    assert(wait_until_zombie_delta_at_most(baseline_zombies, 0UL, 2000UL));
#else
    (void)root;
    (void)shell;
#endif
}

static void assert_posix_sigchld_reaper_preserves_exit_status_during_pty_resume_race(const fs::path& root,
                                                                                     const std::string& shell) {
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
        SessionStore race_store;
        Json waiting = start_test_command(race_store,
                                          "printf 'ready\\n'; IFS= read line; printf 'echo:%s\\n' \"$line\"",
                                          root.string(),
                                          race_shell,
                                          true,
                                          50UL,
                                          DEFAULT_MAX_OUTPUT_TOKENS,
                                          fast_yield,
                                          64UL);
        assert(waiting.at("running").get<bool>());
        std::string waiting_id = waiting.at("daemon_session_id").get<std::string>();
        std::string waiting_output = waiting.at("output").get<std::string>();
        for (int poll = 0; waiting_output.find("ready") == std::string::npos && poll < 20; ++poll) {
            waiting = race_store.write_stdin(
                waiting_id, "", true, 50UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield, false, 0U, 0U);
            assert(waiting.at("running").get<bool>());
            waiting_id = waiting.at("daemon_session_id").get<std::string>();
            waiting_output += waiting.at("output").get<std::string>();
        }
        assert(waiting_output.find("ready") != std::string::npos);

        const Json resumed = race_store.write_stdin(
            waiting_id, "ping\n", true, 1000UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield, false, 0U, 0U);
        assert(!resumed.at("running").get<bool>());
        assert(resumed.at("exit_code").get<int>() == 0);
        assert(resumed.at("output").get<std::string>().find("echo:ping") != std::string::npos);
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
static void assert_non_tty_stdin_closed_rejected(SessionStore& store,
                                                 const fs::path& root,
                                                 const std::string& shell,
                                                 const YieldTimeConfig& yield_time) {
    const Json non_tty_running = start_test_command(store,
                                                    "printf ready; sleep 5",
                                                    root.string(),
                                                    shell,
                                                    false,
                                                    250UL,
                                                    DEFAULT_MAX_OUTPUT_TOKENS,
                                                    yield_time,
                                                    64UL);
    assert(non_tty_running.at("running").get<bool>());
    assert(non_tty_running.at("output").get<std::string>() == "ready");

    bool stdin_closed_rejected = false;
    try {
        (void)store.write_stdin(non_tty_running.at("daemon_session_id").get<std::string>(),
                                "hello\n",
                                true,
                                250UL,
                                DEFAULT_MAX_OUTPUT_TOKENS,
                                yield_time,
                                false,
                                0U,
                                0U);
    } catch (const StdinClosedError& ex) {
        stdin_closed_rejected = std::string(ex.what()).find("stdin is closed") != std::string::npos;
    }
    assert(stdin_closed_rejected);
}

static void assert_tty_resume_round_trip(SessionStore& store,
                                         const fs::path& root,
                                         const std::string& shell) {
    if (!process_session_supports_pty()) {
        return;
    }

    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json waiting = start_test_command(store,
                                            "printf 'ready\\n'; IFS= read line; printf 'echo:%s\\n' \"$line\"",
                                            root.string(),
                                            shell,
                                            true,
                                            50UL,
                                            DEFAULT_MAX_OUTPUT_TOKENS,
                                            fast_yield,
                                            64UL);
    assert(waiting.at("running").get<bool>());
    assert(waiting.at("output").get<std::string>().find("ready") != std::string::npos);

    const std::string waiting_id = waiting.at("daemon_session_id").get<std::string>();
    const Json resumed =
        store.write_stdin(waiting_id, "ping\n", true, 1000UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield, false, 0U, 0U);
    assert(!resumed.at("running").get<bool>());
    assert(resumed.at("exit_code").get<int>() == 0);
    assert(resumed.at("output").get<std::string>().find("echo:ping") != std::string::npos);
}

static void assert_unrelated_sessions_do_not_block_each_other(SessionStore& store,
                                                              const fs::path& root,
                                                              const std::string& shell,
                                                              const YieldTimeConfig& yield_time) {
    if (!process_session_supports_pty()) {
        return;
    }

    const Json slow_running = start_test_command(store,
                                                 "printf slow; sleep 30",
                                                 root.string(),
                                                 shell,
                                                 true,
                                                 250UL,
                                                 DEFAULT_MAX_OUTPUT_TOKENS,
                                                 yield_time,
                                                 64UL);
    assert(slow_running.at("running").get<bool>());

    const Json fast_running = start_test_command(store,
                                                 "IFS= read line; printf '%s' \"$line\"; sleep 30",
                                                 root.string(),
                                                 shell,
                                                 true,
                                                 250UL,
                                                 DEFAULT_MAX_OUTPUT_TOKENS,
                                                 yield_time,
                                                 64UL);
    assert(fast_running.at("running").get<bool>());

    Json slow_poll;
    std::atomic<bool> slow_thread_started(false);
    std::thread slow_thread([&]() {
        slow_thread_started.store(true);
        slow_poll = store.write_stdin(slow_running.at("daemon_session_id").get<std::string>(),
                                      "",
                                      true,
                                      5000UL,
                                      DEFAULT_MAX_OUTPUT_TOKENS,
                                      yield_time,
                                      false,
                                      0U,
                                      0U);
    });

    assert(wait_until_true(slow_thread_started, 1000UL));
    const std::uint64_t fast_started_at = platform::monotonic_ms();
    const Json fast_completed = store.write_stdin(fast_running.at("daemon_session_id").get<std::string>(),
                                                  "ping\n",
                                                  true,
                                                  250UL,
                                                  DEFAULT_MAX_OUTPUT_TOKENS,
                                                  yield_time,
                                                  false,
                                                  0U,
                                                  0U);
    const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
    assert(fast_elapsed_ms < 2000UL && "fast session waited behind unrelated session");
    assert(fast_completed.at("output").get<std::string>().find("ping") != std::string::npos);
    slow_thread.join();
    assert(slow_poll.at("running").get<bool>());
}

static void assert_tty_detection_and_input_round_trip(SessionStore& store,
                                                      const fs::path& root,
                                                      const std::string& shell,
                                                      const YieldTimeConfig& yield_time) {
    if (!process_session_supports_pty()) {
        return;
    }

    const Json tty_running =
        start_test_command(store,
                           "if test -t 0; then printf 'tty:yes\\n'; else printf 'tty:no\\n'; fi; "
                           "IFS= read line; printf 'input:%s\\n' \"$line\"",
                           root.string(),
                           shell,
                           true,
                           250UL,
                           DEFAULT_MAX_OUTPUT_TOKENS,
                           yield_time,
                           64UL);
    assert(tty_running.at("running").get<bool>());
    assert(normalize_output(tty_running.at("output").get<std::string>()) == "tty:yes\n");

    const Json tty_completed = store.write_stdin(tty_running.at("daemon_session_id").get<std::string>(),
                                                 "hello\n",
                                                 true,
                                                 5000UL,
                                                 DEFAULT_MAX_OUTPUT_TOKENS,
                                                 yield_time,
                                                 false,
                                                 0U,
                                                 0U);
    assert(!tty_completed.at("running").get<bool>());
    assert(tty_completed.at("exit_code").get<int>() == 0);
    const std::string normalized_tty_output = normalize_output(tty_completed.at("output").get<std::string>());
    assert(normalized_tty_output.find("hello\n") != std::string::npos);
    assert(normalized_tty_output.find("input:hello\n") != std::string::npos);
}

static void assert_tty_resize_round_trip(SessionStore& store,
                                         const fs::path& root,
                                         const std::string& shell) {
    if (!process_session_supports_pty()) {
        return;
    }

    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json resize_running = start_test_command(store,
                                                   "printf ready; IFS= read line; stty size; sleep 30",
                                                   root.string(),
                                                   shell,
                                                   true,
                                                   50UL,
                                                   DEFAULT_MAX_OUTPUT_TOKENS,
                                                   fast_yield,
                                                   64UL);
    assert(resize_running.at("running").get<bool>());
    const Json resized = store.write_stdin(resize_running.at("daemon_session_id").get<std::string>(),
                                           "\n",
                                           true,
                                           1000UL,
                                           DEFAULT_MAX_OUTPUT_TOKENS,
                                           fast_yield,
                                           true,
                                           33U,
                                           101U);
    assert(resized.at("running").get<bool>());
    assert(normalize_output(resized.at("output").get<std::string>()).find("33 101") != std::string::npos);
}

static void assert_non_tty_resize_rejected(SessionStore& store,
                                           const fs::path& root,
                                           const std::string& shell,
                                           const YieldTimeConfig& yield_time) {
    if (!process_session_supports_pty()) {
        return;
    }

    const Json non_tty_running = start_test_command(store,
                                                    "printf ready; sleep 5",
                                                    root.string(),
                                                    shell,
                                                    false,
                                                    250UL,
                                                    DEFAULT_MAX_OUTPUT_TOKENS,
                                                    yield_time,
                                                    64UL);
    assert(non_tty_running.at("running").get<bool>());

    bool non_tty_resize_rejected = false;
    try {
        (void)store.write_stdin(non_tty_running.at("daemon_session_id").get<std::string>(),
                                "",
                                true,
                                250UL,
                                DEFAULT_MAX_OUTPUT_TOKENS,
                                yield_time,
                                true,
                                33U,
                                101U);
    } catch (const ProcessPtyResizeUnsupportedError& ex) {
        non_tty_resize_rejected = std::string(ex.what()).find("requires a tty session") != std::string::npos;
    }
    assert(non_tty_resize_rejected);
}

static void assert_session_limit_prunes_oldest_running(const fs::path& root, const std::string& shell) {
    SessionStore limit_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json first_running = start_test_command(limit_store,
                                                  "printf 'first'; sleep 30",
                                                  root.string(),
                                                  shell,
                                                  false,
                                                  1UL,
                                                  DEFAULT_MAX_OUTPUT_TOKENS,
                                                  fast_yield,
                                                  2UL);
    const Json second_running = start_test_command(limit_store,
                                                   "printf 'second'; sleep 30",
                                                   root.string(),
                                                   shell,
                                                   false,
                                                   1UL,
                                                   DEFAULT_MAX_OUTPUT_TOKENS,
                                                   fast_yield,
                                                   2UL);
    const Json third_running = start_test_command(limit_store,
                                                  "printf 'third'; sleep 30",
                                                  root.string(),
                                                  shell,
                                                  false,
                                                  1UL,
                                                  DEFAULT_MAX_OUTPUT_TOKENS,
                                                  fast_yield,
                                                  2UL);
    assert(first_running.at("running").get<bool>());
    assert(second_running.at("running").get<bool>());
    assert(third_running.at("running").get<bool>());
    assert_unknown_session(limit_store, first_running.at("daemon_session_id").get<std::string>(), fast_yield);
    assert(limit_store
               .write_stdin(second_running.at("daemon_session_id").get<std::string>(),
                            "",
                            true,
                            1UL,
                            DEFAULT_MAX_OUTPUT_TOKENS,
                            fast_yield,
                            false,
                            0U,
                            0U)
               .at("running")
               .get<bool>());
    assert(limit_store
               .write_stdin(third_running.at("daemon_session_id").get<std::string>(),
                            "",
                            true,
                            1UL,
                            DEFAULT_MAX_OUTPUT_TOKENS,
                            fast_yield,
                            false,
                            0U,
                            0U)
               .at("running")
               .get<bool>());
}

static void assert_recent_session_survives_limit_prune(const fs::path& root, const std::string& shell) {
    SessionStore recency_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json first_running = start_test_command(recency_store,
                                                  "printf 'first'; sleep 30",
                                                  root.string(),
                                                  shell,
                                                  false,
                                                  1UL,
                                                  DEFAULT_MAX_OUTPUT_TOKENS,
                                                  fast_yield,
                                                  2UL);
    const Json second_running = start_test_command(recency_store,
                                                   "printf 'second'; sleep 30",
                                                   root.string(),
                                                   shell,
                                                   false,
                                                   1UL,
                                                   DEFAULT_MAX_OUTPUT_TOKENS,
                                                   fast_yield,
                                                   2UL);
    const Json first_touch = recency_store.write_stdin(first_running.at("daemon_session_id").get<std::string>(),
                                                       "",
                                                       true,
                                                       1UL,
                                                       DEFAULT_MAX_OUTPUT_TOKENS,
                                                       fast_yield,
                                                       false,
                                                       0U,
                                                       0U);
    assert(first_touch.at("running").get<bool>());

    const Json third_running = start_test_command(recency_store,
                                                  "printf 'third'; sleep 30",
                                                  root.string(),
                                                  shell,
                                                  false,
                                                  1UL,
                                                  DEFAULT_MAX_OUTPUT_TOKENS,
                                                  fast_yield,
                                                  2UL);
    assert(third_running.at("running").get<bool>());
    assert_unknown_session(recency_store, second_running.at("daemon_session_id").get<std::string>(), fast_yield);
    assert(recency_store
               .write_stdin(first_running.at("daemon_session_id").get<std::string>(),
                            "",
                            true,
                            1UL,
                            DEFAULT_MAX_OUTPUT_TOKENS,
                            fast_yield,
                            false,
                            0U,
                            0U)
               .at("running")
               .get<bool>());
}

static void assert_exited_session_is_pruned_before_live_session(const fs::path& root, const std::string& shell) {
    SessionStore exited_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    const Json exited_running =
        start_test_command(exited_store, "sleep 0.05", root.string(), shell, false, 1UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield, 2UL);
    const Json live_running = start_test_command(exited_store,
                                                 "printf 'live'; sleep 30",
                                                 root.string(),
                                                 shell,
                                                 false,
                                                 1UL,
                                                 DEFAULT_MAX_OUTPUT_TOKENS,
                                                 fast_yield,
                                                 2UL);
    assert(exited_running.at("running").get<bool>());
    assert(live_running.at("running").get<bool>());
    assert(wait_until_session_exits(
        exited_store, exited_running.at("daemon_session_id").get<std::string>(), fast_yield, 2000UL));

    const Json replacement_running = start_test_command(exited_store,
                                                        "printf 'replacement'; sleep 30",
                                                        root.string(),
                                                        shell,
                                                        false,
                                                        1UL,
                                                        DEFAULT_MAX_OUTPUT_TOKENS,
                                                        fast_yield,
                                                        2UL);
    assert(replacement_running.at("running").get<bool>());
    assert_unknown_session(exited_store, exited_running.at("daemon_session_id").get<std::string>(), fast_yield);
    assert(exited_store
               .write_stdin(live_running.at("daemon_session_id").get<std::string>(),
                            "",
                            true,
                            1UL,
                            DEFAULT_MAX_OUTPUT_TOKENS,
                            fast_yield,
                            false,
                            0U,
                            0U)
               .at("running")
               .get<bool>());
}

static void assert_recent_session_is_protected_from_prune(const fs::path& root, const std::string& shell) {
    SessionStore protected_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    std::vector<std::string> daemon_session_ids;
    for (int index = 0; index < 10; ++index) {
        const Json running = start_test_command(protected_store,
                                                "printf ready; sleep 30",
                                                root.string(),
                                                shell,
                                                false,
                                                1UL,
                                                DEFAULT_MAX_OUTPUT_TOKENS,
                                                fast_yield,
                                                10UL);
        assert(running.at("running").get<bool>());
        daemon_session_ids.push_back(running.at("daemon_session_id").get<std::string>());
    }

    assert(protected_store
               .write_stdin(daemon_session_ids[0], "", true, 1UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield, false, 0U, 0U)
               .at("running")
               .get<bool>());

    const Json protected_replacement = start_test_command(protected_store,
                                                          "printf extra; sleep 30",
                                                          root.string(),
                                                          shell,
                                                          false,
                                                          1UL,
                                                          DEFAULT_MAX_OUTPUT_TOKENS,
                                                          fast_yield,
                                                          10UL);
    assert(protected_replacement.at("running").get<bool>());
    assert_unknown_session(protected_store, daemon_session_ids[1], fast_yield);
    assert(protected_store
               .write_stdin(daemon_session_ids[0], "", true, 1UL, DEFAULT_MAX_OUTPUT_TOKENS, fast_yield, false, 0U, 0U)
               .at("running")
               .get<bool>());
}
#endif

static void assert_stdin_and_tty_behavior(SessionStore& store,
                                          const fs::path& root,
                                          const std::string& shell,
                                          const YieldTimeConfig& yield_time) {
#ifndef _WIN32
    assert_non_tty_stdin_closed_rejected(store, root, shell, yield_time);
    assert_tty_resume_round_trip(store, root, shell);
    assert_unrelated_sessions_do_not_block_each_other(store, root, shell, yield_time);
    assert_tty_detection_and_input_round_trip(store, root, shell, yield_time);
    assert_tty_resize_round_trip(store, root, shell);
    assert_non_tty_resize_rejected(store, root, shell, yield_time);
#else
    const Json xp_running = start_test_command(store,
                                               "echo ready&set /P line=&call echo got:%line%",
                                               root.string(),
                                               shell,
                                               false,
                                               250UL,
                                               DEFAULT_MAX_OUTPUT_TOKENS,
                                               yield_time,
                                               64UL);
    assert(xp_running.at("running").get<bool>());
    const std::string xp_initial = normalize_output(xp_running.at("output").get<std::string>());

    const Json xp_completed = store.write_stdin(xp_running.at("daemon_session_id").get<std::string>(),
                                                "hello\r\n",
                                                true,
                                                5000UL,
                                                DEFAULT_MAX_OUTPUT_TOKENS,
                                                yield_time,
                                                false,
                                                0U,
                                                0U);
    assert(!xp_completed.at("running").get<bool>());
    assert(xp_completed.at("exit_code").get<int>() == 0);
    const std::string xp_output = xp_initial + normalize_output(xp_completed.at("output").get<std::string>());
    assert(xp_output.find("ready\n") != std::string::npos);
    assert(xp_output.find("got:hello\n") != std::string::npos);
#endif
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
#endif
}

static void assert_threshold_warnings_and_unknown_sessions(SessionStore& store,
                                                           const fs::path& root,
                                                           const std::string& shell,
                                                           const YieldTimeConfig& yield_time) {
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
            const Json running = start_test_command(warning_store,
                                                    "printf ready; sleep 30",
                                                    root.string(),
                                                    shell,
                                                    false,
                                                    1UL,
                                                    DEFAULT_MAX_OUTPUT_TOKENS,
                                                    fast_yield,
                                                    DEFAULT_MAX_OPEN_SESSIONS);
            assert(running.at("running").get<bool>());
            if (index + 1UL < threshold) {
                assert(running.at("warnings").empty());
            } else {
                threshold_response = running;
            }
        }
        assert(threshold_response.at("warnings").size() == 1U);
        assert(threshold_response.at("warnings")[0].at("code").get<std::string>() == "exec_session_limit_approaching");
        assert(threshold_response.at("warnings")[0].at("message").get<std::string>() ==
               "Target `cpp-test` now has " + std::to_string(threshold) + " open exec sessions.");
    }

    bool unknown_session_rejected = false;
    try {
        (void)store.write_stdin(
            "missing-session", "", true, 250UL, DEFAULT_MAX_OUTPUT_TOKENS, yield_time, false, 0U, 0U);
    } catch (const UnknownSessionError&) {
        unknown_session_rejected = true;
    }
    assert(unknown_session_rejected);
#endif
}

static void assert_threshold_warnings_follow_configured_limit(const fs::path& root,
                                                              const std::string& shell) {
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
        const Json running = start_test_command(warning_store,
                                                "printf ready; sleep 30",
                                                root.string(),
                                                shell,
                                                false,
                                                1UL,
                                                DEFAULT_MAX_OUTPUT_TOKENS,
                                                fast_yield,
                                                max_open_sessions);
        assert(running.at("running").get<bool>());
        if (index + 1UL < threshold) {
            assert(running.at("warnings").empty());
        } else {
            threshold_response = running;
        }
    }
    assert(threshold_response.at("warnings").size() == 1U);
    assert(threshold_response.at("warnings")[0].at("code").get<std::string>() == "exec_session_limit_approaching");
    assert(threshold_response.at("warnings")[0].at("message").get<std::string>() ==
           "Target `cpp-test` now has " + std::to_string(threshold) + " open exec sessions.");
#endif
}

int main() {
#ifndef _WIN32
    install_posix_child_reaper();
#endif
    const fs::path root = make_test_root();
    SessionStore store;
    const YieldTimeConfig yield_time = YieldTimeConfig();
    const std::string shell = platform::resolve_default_shell("");

    assert_completed_command_output(store, root, shell, yield_time);
    assert_token_limiting(store, root, shell, yield_time);
    assert_posix_locale_and_late_output(store, root, shell, yield_time);
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
