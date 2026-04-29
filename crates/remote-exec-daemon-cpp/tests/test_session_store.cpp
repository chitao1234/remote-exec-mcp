#include <cassert>
#include <filesystem>
#include <string>

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

    const Json response = store.start_command(
        merge_command,
        root.string(),
        shell,
        false,
        false,
        true,
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

    const Json token_limited = store.start_command(
        token_command,
        root.string(),
        shell,
        false,
        false,
        true,
        5000UL,
        2UL,
        yield_time,
        64UL
    );
    assert(token_limited.at("original_token_count").get<unsigned long>() == 3UL);
    assert(normalize_output(token_limited.at("output").get<std::string>()) == "one two");

    const Json zero_limited = store.start_command(
        token_command,
        root.string(),
        shell,
        false,
        false,
        true,
        5000UL,
        0UL,
        yield_time,
        64UL
    );
    assert(zero_limited.at("original_token_count").get<unsigned long>() == 3UL);
    assert(zero_limited.at("output").get<std::string>().empty());

#ifndef _WIN32
    const Json locale_response = store.start_command(
        "printf '%s %s\\n' \"$LC_ALL\" \"$LANG\"",
        root.string(),
        shell,
        false,
        false,
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    assert(locale_response.at("exit_code").get<int>() == 0);
    assert(locale_response.at("output").get<std::string>() == "C.UTF-8 C.UTF-8\n");

    const Json running = store.start_command(
        "printf ready; IFS= read line; printf ' got:%s\\n' \"$line\"",
        root.string(),
        shell,
        false,
        false,
        true,
        250UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    assert(running.at("running").get<bool>());
    assert(running.at("output").get<std::string>() == "ready");

    const Json completed = store.write_stdin(
        running.at("daemon_session_id").get<std::string>(),
        "hello\n",
        true,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time
    );
    assert(!completed.at("running").get<bool>());
    assert(completed.at("exit_code").get<int>() == 0);
    assert(completed.at("output").get<std::string>() == " got:hello\n");

    if (process_session_supports_pty()) {
        const Json tty_running = store.start_command(
            "if test -t 0; then printf 'tty:yes\\n'; else printf 'tty:no\\n'; fi; "
            "IFS= read line; printf 'input:%s\\n' \"$line\"",
            root.string(),
            shell,
            false,
            true,
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
#endif

    return 0;
}
