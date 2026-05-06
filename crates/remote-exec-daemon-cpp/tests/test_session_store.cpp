#include <cassert>
#include <filesystem>
#include <string>
#include <thread>
#include <vector>

#include "config.h"
#include "platform.h"
#include "process_session.h"
#include "session_store.h"

namespace fs = std::filesystem;

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
    return store.start_command(
        "cpp-test",
        command,
        workdir,
        shell,
        false,
        tty,
        true,
        yield_time_ms,
        max_output_tokens,
        yield_time,
        max_open_sessions
    );
}

static YieldTimeConfig fast_yield_time_config() {
    YieldTimeConfig config;
    config.exec_command = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    config.write_stdin_poll = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    config.write_stdin_input = YieldTimeOperationConfig{1UL, 1000UL, 1UL};
    return config;
}

static void assert_unknown_session(
    SessionStore& store,
    const std::string& daemon_session_id,
    const YieldTimeConfig& yield_time
) {
    bool rejected = false;
    try {
        (void)store.write_stdin(
            daemon_session_id,
            "",
            true,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time
        );
    } catch (const UnknownSessionError&) {
        rejected = true;
    }
    assert(rejected);
}

int main() {
    const fs::path root = make_test_root();
    SessionStore store;
    const YieldTimeConfig yield_time = default_yield_time_config();
    const std::string shell = platform::resolve_default_shell("");

#ifdef _WIN32
    const std::string merge_command =
        "echo stdout-1 & echo stderr-1 1>&2 & echo stdout-2 & echo stderr-2 1>&2";
#else
    const std::string merge_command =
        "printf 'stdout-1\\n'; printf 'stderr-1\\n' >&2; "
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

    assert(response.at("daemon_session_id").is_null());
    assert(!response.at("running").get<bool>());
    assert(response.at("exit_code").get<int>() == 0);
    assert(
        normalize_output(response.at("output").get<std::string>()) ==
        "stdout-1\nstderr-1\nstdout-2\nstderr-2\n"
    );

#ifdef _WIN32
    const std::string token_command = "echo one two three";
#else
    const std::string token_command = "printf 'one two three\\n'";
#endif

    const Json token_limited = start_test_command(
        store,
        token_command,
        root.string(),
        shell,
        false,
        5000UL,
        2UL,
        yield_time,
        64UL
    );
    assert(token_limited.at("original_token_count").get<unsigned long>() == 3UL);
    assert(normalize_output(token_limited.at("output").get<std::string>()) == "one two");

    const Json zero_limited = start_test_command(
        store,
        token_command,
        root.string(),
        shell,
        false,
        5000UL,
        0UL,
        yield_time,
        64UL
    );
    assert(zero_limited.at("original_token_count").get<unsigned long>() == 3UL);
    assert(zero_limited.at("output").get<std::string>().empty());

#ifndef _WIN32
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
    assert(locale_response.at("exit_code").get<int>() == 0);
    assert(locale_response.at("output").get<std::string>() == "C.UTF-8 C.UTF-8\n");

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
    assert(!late_output.at("running").get<bool>());
    assert(late_output.at("exit_code").get<int>() == 0);
    assert(late_output.at("output").get<std::string>() == "late tail");

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
    assert(newline_preserved.at("original_token_count").get<unsigned long>() == 2UL);
    assert(newline_preserved.at("output").get<std::string>() == "one two\n");

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
    assert(!stdin_closed_response.at("running").get<bool>());
    assert(stdin_closed_response.at("exit_code").get<int>() == 0);
    assert(stdin_closed_response.at("output").get<std::string>() == "stdin:closed\n");

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
    assert(non_tty_running.at("running").get<bool>());
    assert(non_tty_running.at("output").get<std::string>() == "ready");

    bool stdin_closed_rejected = false;
    try {
        (void)store.write_stdin(
            non_tty_running.at("daemon_session_id").get<std::string>(),
            "hello\n",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time
        );
    } catch (const StdinClosedError& ex) {
        stdin_closed_rejected =
            std::string(ex.what()).find("stdin is closed") != std::string::npos;
    }
    assert(stdin_closed_rejected);

    if (process_session_supports_pty()) {
        const YieldTimeConfig fast_yield = fast_yield_time_config();
        const Json waiting = start_test_command(
            store,
            "printf 'ready\\n'; IFS= read line; printf 'echo:%s\\n' \"$line\"",
            root.string(),
            shell,
            true,
            50UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            64UL
        );
        assert(waiting.at("running").get<bool>());
        assert(waiting.at("output").get<std::string>().find("ready") != std::string::npos);
        const std::string waiting_id = waiting.at("daemon_session_id").get<std::string>();
        const Json resumed = store.write_stdin(
            waiting_id,
            "ping\n",
            true,
            1000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield
        );
        assert(!resumed.at("running").get<bool>());
        assert(resumed.at("exit_code").get<int>() == 0);
        assert(resumed.at("output").get<std::string>().find("echo:ping") != std::string::npos);

        const Json slow_running = start_test_command(
            store,
            "printf slow; sleep 30",
            root.string(),
            shell,
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        assert(slow_running.at("running").get<bool>());

        const Json fast_running = start_test_command(
            store,
            "IFS= read line; printf '%s' \"$line\"; sleep 30",
            root.string(),
            shell,
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        assert(fast_running.at("running").get<bool>());

        Json slow_poll;
        std::thread slow_thread([&]() {
            slow_poll = store.write_stdin(
                slow_running.at("daemon_session_id").get<std::string>(),
                "",
                true,
                5000UL,
                DEFAULT_MAX_OUTPUT_TOKENS,
                yield_time
            );
        });

        platform::sleep_ms(200);
        const std::uint64_t fast_started_at = platform::monotonic_ms();
        const Json fast_completed = store.write_stdin(
            fast_running.at("daemon_session_id").get<std::string>(),
            "ping\n",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time
        );
        const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
        assert(
            fast_elapsed_ms < 2000UL &&
            "fast session waited behind unrelated session"
        );
        assert(fast_completed.at("output").get<std::string>().find("ping") != std::string::npos);
        slow_thread.join();
        assert(slow_poll.at("running").get<bool>());

        const Json tty_running = start_test_command(
            store,
            "if test -t 0; then printf 'tty:yes\\n'; else printf 'tty:no\\n'; fi; "
            "IFS= read line; printf 'input:%s\\n' \"$line\"",
            root.string(),
            shell,
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        assert(tty_running.at("running").get<bool>());
        assert(normalize_output(tty_running.at("output").get<std::string>()) == "tty:yes\n");

        const Json tty_completed = store.write_stdin(
            tty_running.at("daemon_session_id").get<std::string>(),
            "hello\n",
            true,
            5000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time
        );
        assert(!tty_completed.at("running").get<bool>());
        assert(tty_completed.at("exit_code").get<int>() == 0);
        const std::string normalized_tty_output =
            normalize_output(tty_completed.at("output").get<std::string>());
        assert(normalized_tty_output.find("hello\n") != std::string::npos);
        assert(normalized_tty_output.find("input:hello\n") != std::string::npos);
    }

    {
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
        assert(first_running.at("running").get<bool>());
        assert(second_running.at("running").get<bool>());
        assert(third_running.at("running").get<bool>());
        assert_unknown_session(
            limit_store,
            first_running.at("daemon_session_id").get<std::string>(),
            fast_yield
        );
        assert(
            limit_store
                .write_stdin(
                    second_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield
                )
                .at("running")
                .get<bool>()
        );
        assert(
            limit_store
                .write_stdin(
                    third_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield
                )
                .at("running")
                .get<bool>()
        );
    }

    {
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
        const Json first_touch = recency_store.write_stdin(
            first_running.at("daemon_session_id").get<std::string>(),
            "",
            true,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield
        );
        assert(first_touch.at("running").get<bool>());

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
        assert(third_running.at("running").get<bool>());
        assert_unknown_session(
            recency_store,
            second_running.at("daemon_session_id").get<std::string>(),
            fast_yield
        );
        assert(
            recency_store
                .write_stdin(
                    first_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield
                )
                .at("running")
                .get<bool>()
        );
    }

    {
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
        assert(exited_running.at("running").get<bool>());
        assert(live_running.at("running").get<bool>());
        platform::sleep_ms(150UL);

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
        assert(replacement_running.at("running").get<bool>());
        assert_unknown_session(
            exited_store,
            exited_running.at("daemon_session_id").get<std::string>(),
            fast_yield
        );
        assert(
            exited_store
                .write_stdin(
                    live_running.at("daemon_session_id").get<std::string>(),
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield
                )
                .at("running")
                .get<bool>()
        );
    }

    {
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
            assert(running.at("running").get<bool>());
            daemon_session_ids.push_back(
                running.at("daemon_session_id").get<std::string>()
            );
        }

        assert(
            protected_store
                .write_stdin(
                    daemon_session_ids[0],
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield
                )
                .at("running")
                .get<bool>()
        );

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
        assert(protected_replacement.at("running").get<bool>());
        assert_unknown_session(protected_store, daemon_session_ids[1], fast_yield);
        assert(
            protected_store
                .write_stdin(
                    daemon_session_ids[0],
                    "",
                    true,
                    1UL,
                    DEFAULT_MAX_OUTPUT_TOKENS,
                    fast_yield
                )
                .at("running")
                .get<bool>()
        );
    }

    {
        SessionStore warning_store;
        const YieldTimeConfig fast_yield = fast_yield_time_config();
        Json threshold_response;
        for (int index = 0; index < 60; ++index) {
            const Json running = start_test_command(
                warning_store,
                "printf ready; sleep 30",
                root.string(),
                shell,
                false,
                1UL,
                DEFAULT_MAX_OUTPUT_TOKENS,
                fast_yield,
                64UL
            );
            assert(running.at("running").get<bool>());
            if (index < 59) {
                assert(running.at("warnings").empty());
            } else {
                threshold_response = running;
            }
        }
        assert(threshold_response.at("warnings").size() == 1U);
        assert(
            threshold_response.at("warnings")[0].at("code").get<std::string>() ==
            "exec_session_limit_approaching"
        );
        assert(
            threshold_response.at("warnings")[0].at("message").get<std::string>() ==
            "Target `cpp-test` now has 60 open exec sessions."
        );
    }

    bool unknown_session_rejected = false;
    try {
        (void)store.write_stdin(
            "missing-session",
            "",
            true,
            250UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time
        );
    } catch (const UnknownSessionError&) {
        unknown_session_rejected = true;
    }
    assert(unknown_session_rejected);
#else
    const Json xp_running = start_test_command(
        store,
        "echo ready&set /P line=&call echo got:%line%",
        root.string(),
        shell,
        false,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    assert(xp_running.at("running").get<bool>());
    const std::string xp_initial =
        normalize_output(xp_running.at("output").get<std::string>());

    const Json xp_completed = store.write_stdin(
        xp_running.at("daemon_session_id").get<std::string>(),
        "hello\r\n",
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time
    );
    assert(!xp_completed.at("running").get<bool>());
    assert(xp_completed.at("exit_code").get<int>() == 0);
    const std::string xp_output =
        xp_initial + normalize_output(xp_completed.at("output").get<std::string>());
    assert(xp_output.find("ready\n") != std::string::npos);
    assert(xp_output.find("got:hello\n") != std::string::npos);
#endif

    return 0;
}
