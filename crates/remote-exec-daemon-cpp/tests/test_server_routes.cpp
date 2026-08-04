#include "test_assert.h"
#include <cctype>
#include <cstdint>
#include <string>
#include <thread>

#include "exec/process_session.h"
#include "http/http_helpers.h"
#include "platform/platform.h"
#include "rpc/server_routes.h"
#include "test_pty_helpers.h"
#include "test_server_routes_shared.h"

namespace fs = test_fs;

namespace {

#ifdef _WIN32
const unsigned long WINDOWS_LIVE_SESSION_SLEEP_SECONDS = 8UL;
const unsigned long WINDOWS_RESIZE_HELPER_SLEEP_SECONDS = 2UL;
#endif

} // namespace

static std::string normalize_output(const std::string& input) {
    std::string output;
    output.reserve(input.size());
    for (std::string::const_iterator it = input.begin(); it != input.end(); ++it) {
        if (*it != '\r') {
            output.push_back(*it);
        }
    }
    return output;
}

static std::string trim_trailing_exec_output(std::string output) {
    while (!output.empty()) {
        const char ch = output[output.size() - 1];
        if (ch != '\n' && ch != ' ' && ch != '\t') {
            break;
        }
        output.erase(output.size() - 1);
    }
    return output;
}

#ifdef _WIN32
static bool contains_terminal_escape(const std::string& input) {
    return input.find('\x1b') != std::string::npos;
}
#endif

static Json exec_write_json(
    TestRouteHarness& harness,
    const std::string& daemon_session_id,
    const std::string& chars,
    unsigned long yield_time_ms,
    bool has_pty_size = false,
    unsigned short rows = 0U,
    unsigned short cols = 0U
) {
    Json body{
        {"daemon_session_id", daemon_session_id},
        {"chars", chars},
        {"yield_time_ms", yield_time_ms},
    };
    if (has_pty_size) {
        body["pty_size"] = Json{{"rows", rows}, {"cols", cols}};
    }
    const HttpResponse response =
        route_request(harness, make_json_http_request("/v1/exec/write", body));
    TEST_ASSERT(response.status == 200);
    return Json::parse(response.body);
}

template <typename MatchPredicate>
static std::string append_running_exec_output_until(
    TestRouteHarness& harness,
    const std::string& daemon_session_id,
    std::string output,
    MatchPredicate matches,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (!matches(output) && platform::monotonic_ms() - started < timeout_ms) {
        const Json poll = exec_write_json(harness, daemon_session_id, "", 250UL);
        output += normalize_output(poll.at("output").get<std::string>());
        if (!poll.at("running").get<bool>()) {
            break;
        }
        platform::sleep_ms(10UL);
    }
    return output;
}

static Json poll_exec_until_done(
    TestRouteHarness& harness,
    const std::string& daemon_session_id,
    Json response,
    std::string* output,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (response.at("running").get<bool>() && platform::monotonic_ms() - started < timeout_ms) {
        platform::sleep_ms(10UL);
        response = exec_write_json(harness, daemon_session_id, "", 250UL);
        if (output != nullptr) {
            output->append(normalize_output(response.at("output").get<std::string>()));
        }
    }
    return response;
}

static std::string long_running_non_tty_command() {
#ifdef _WIN32
    return test_exec_pty::windows_stdin_echo_sleep_helper_command(
        "--server-routes-helper",
        "ignored",
        WINDOWS_LIVE_SESSION_SLEEP_SECONDS
    );
#else
    return "printf ready; sleep 5";
#endif
}

static std::string slow_tty_command() {
#ifdef _WIN32
    return "echo slow&"
           + test_exec_pty::windows_ping_sleep_command(WINDOWS_LIVE_SESSION_SLEEP_SECONDS);
#else
    return "printf slow; sleep 30";
#endif
}

static std::string fast_tty_file_command() {
#ifdef _WIN32
    return test_exec_pty::windows_stdin_file_helper_command(
        "--server-routes-helper",
        "fast-session-input.txt",
        WINDOWS_LIVE_SESSION_SLEEP_SECONDS
    );
#else
    return "IFS= read line; printf '%s' \"$line\" > fast-session-input.txt; sleep 30";
#endif
}

static std::string tty_round_trip_command() {
#ifdef _WIN32
    return test_exec_pty::windows_tty_flag_helper_command(
        "--server-routes-helper",
        "tty-detected.txt"
    );
#else
    return "if test -t 0; then printf 'tty:yes\\n'; else printf 'tty:no\\n'; fi; IFS= read line; "
           "printf 'input:%s\\n' "
           "\"$line\"";
#endif
}

static std::string tty_resize_command() {
#ifdef _WIN32
    return test_exec_pty::windows_resize_helper_command(
        "--server-routes-helper",
        WINDOWS_RESIZE_HELPER_SLEEP_SECONDS
    );
#else
    return "stty -a; printf ready; IFS= read line; stty -a; sleep 30";
#endif
}

static void assert_exec_routes(TestRouteHarness& harness, const fs::path& root) {
    const HttpResponse missing_cmd_response = route_request(
        harness,
        make_json_http_request("/v1/exec/start", Json{{"workdir", root.string()}})
    );
    TEST_ASSERT(missing_cmd_response.status == 400);
    TEST_ASSERT(
        Json::parse(missing_cmd_response.body).at("code").get<std::string>() == "bad_request"
    );

    const HttpResponse non_tty_start_response = route_request(
        harness,
        make_json_http_request(
            "/v1/exec/start",
            Json{
                {"cmd", long_running_non_tty_command()},
                {"workdir", root.string()},
                {"login", false},
                {"tty", false},
                {"yield_time_ms", 250},
            }
        )
    );
    TEST_ASSERT(non_tty_start_response.status == 200);
    const Json non_tty_started = Json::parse(non_tty_start_response.body);
    TEST_ASSERT(non_tty_started.at("running").get<bool>());
    std::string non_tty_output = normalize_output(non_tty_started.at("output").get<std::string>());
    if (non_tty_output.find("ready") == std::string::npos) {
        non_tty_output = append_running_exec_output_until(
            harness,
            non_tty_started.at("daemon_session_id").get<std::string>(),
            non_tty_output,
            [&](const std::string& value) { return value.find("ready") != std::string::npos; },
            2000UL
        );
    }
    TEST_ASSERT(trim_trailing_exec_output(non_tty_output).find("ready") != std::string::npos);

#ifdef _WIN32
    const HttpResponse stdin_write_response = route_request(
        harness,
        make_json_http_request(
            "/v1/exec/write",
            Json{
                {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                {"chars", "hello\n"},
                {"yield_time_ms", 250},
            }
        )
    );
    TEST_ASSERT(stdin_write_response.status == 200);
    TEST_ASSERT(Json::parse(stdin_write_response.body).at("running").get<bool>());
#else
    const HttpResponse stdin_closed_response = route_request(
        harness,
        make_json_http_request(
            "/v1/exec/write",
            Json{
                {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                {"chars", "hello\n"},
                {"yield_time_ms", 250},
            }
        )
    );
    TEST_ASSERT(stdin_closed_response.status == 400);
    TEST_ASSERT(
        Json::parse(stdin_closed_response.body).at("code").get<std::string>() == "stdin_closed"
    );
#endif

    const HttpResponse invalid_pty_size_response = route_request(
        harness,
        make_json_http_request(
            "/v1/exec/write",
            Json{
                {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                {"chars", ""},
                {"yield_time_ms", 250},
                {"pty_size", Json{{"rows", 0}, {"cols", 80}}},
            }
        )
    );
    TEST_ASSERT(invalid_pty_size_response.status == 400);
    TEST_ASSERT(
        Json::parse(invalid_pty_size_response.body).at("code").get<std::string>()
        == "invalid_pty_size"
    );

    const HttpResponse non_tty_resize_response = route_request(
        harness,
        make_json_http_request(
            "/v1/exec/write",
            Json{
                {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                {"chars", ""},
                {"yield_time_ms", 250},
                {"pty_size", Json{{"rows", 33}, {"cols", 101}}},
            }
        )
    );
    TEST_ASSERT(non_tty_resize_response.status == 400);
    TEST_ASSERT(
        Json::parse(non_tty_resize_response.body).at("code").get<std::string>() == "tty_unsupported"
    );

    const HttpResponse invalid_session_id_type_response = route_request(
        harness,
        make_json_http_request(
            "/v1/exec/write",
            Json{
                {"daemon_session_id", Json{{"unexpected", true}}},
                {"chars", ""},
                {"yield_time_ms", 250},
            }
        )
    );
    TEST_ASSERT(invalid_session_id_type_response.status == 400);
    TEST_ASSERT(
        Json::parse(invalid_session_id_type_response.body).at("code").get<std::string>()
        == "bad_request"
    );

    if (!test_exec_pty::should_skip_pty_tests(process_session_supports_pty())) {
        const HttpResponse slow_start_response = route_request(
            harness,
            make_json_http_request(
                "/v1/exec/start",
                Json{
                    {"cmd", slow_tty_command()},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        );
        TEST_ASSERT(slow_start_response.status == 200);
        const Json slow_started = Json::parse(slow_start_response.body);
        TEST_ASSERT(slow_started.at("running").get<bool>());

        const fs::path fast_input_path = root / "fast-session-input.txt";
        fs::remove_all(fast_input_path);
        const HttpResponse fast_start_response = route_request(
            harness,
            make_json_http_request(
                "/v1/exec/start",
                Json{
                    {"cmd", fast_tty_file_command()},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        );
        TEST_ASSERT(fast_start_response.status == 200);
        const Json fast_started = Json::parse(fast_start_response.body);
        TEST_ASSERT(fast_started.at("running").get<bool>());

        HttpResponse slow_poll_response;
        std::thread slow_thread([&]() {
            slow_poll_response = route_request(
                harness,
                make_json_http_request(
                    "/v1/exec/write",
                    Json{
                        {"daemon_session_id",
                         slow_started.at("daemon_session_id").get<std::string>()},
                        {"chars", ""},
                        {"yield_time_ms", 5000},
                    }
                )
            );
        });

        platform::sleep_ms(200);
        const std::uint64_t fast_started_at = platform::monotonic_ms();
        const HttpResponse fast_write_response = route_request(
            harness,
            make_json_http_request(
                "/v1/exec/write",
                Json{
                    {"daemon_session_id", fast_started.at("daemon_session_id").get<std::string>()},
                    {"chars", test_exec_pty::terminal_input_line("ping")},
                    {"yield_time_ms", 250},
                }
            )
        );
        const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
        TEST_ASSERT(fast_write_response.status == 200);
        TEST_ASSERT(
            fast_elapsed_ms < 2000UL && "fast route request waited behind unrelated session"
        );
        TEST_ASSERT(test_exec_pty::wait_until_file_contains(fast_input_path, "ping", 2000UL));
        slow_thread.join();
        TEST_ASSERT(slow_poll_response.status == 200);

        const HttpResponse start_response = route_request(
            harness,
            make_json_http_request(
                "/v1/exec/start",
                Json{
                    {"cmd", tty_round_trip_command()},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 1000},
                }
            )
        );
        TEST_ASSERT(start_response.status == 200);
        const Json started = Json::parse(start_response.body);
        TEST_ASSERT(started.at("running").get<bool>());
        std::string start_output = normalize_output(started.at("output").get<std::string>());
        if (start_output.find("tty:yes\n") == std::string::npos) {
            start_output = append_running_exec_output_until(
                harness,
                started.at("daemon_session_id").get<std::string>(),
                start_output,
                [&](const std::string& value) {
                    return value.find("tty:yes\n") != std::string::npos;
                },
                2000UL
            );
        }
        TEST_ASSERT(start_output.find("tty:yes\n") != std::string::npos);

        const std::string started_session_id = started.at("daemon_session_id").get<std::string>();
        Json completed = exec_write_json(
            harness,
            started_session_id,
            test_exec_pty::terminal_input_line("hello"),
            5000UL
        );
        std::string output =
            start_output + normalize_output(completed.at("output").get<std::string>());
        completed = poll_exec_until_done(harness, started_session_id, completed, &output, 5000UL);
        TEST_ASSERT(!completed.at("running").get<bool>());
        TEST_ASSERT(completed.at("exit_code").get<int>() == 0);
        TEST_ASSERT(output.find("tty:yes\n") != std::string::npos);
        TEST_ASSERT(output.find("hello\n") != std::string::npos);
        TEST_ASSERT(output.find("input:hello\n") != std::string::npos);
#ifdef _WIN32
        TEST_ASSERT(!contains_terminal_escape(output));
#endif

        const HttpResponse resize_start_response = route_request(
            harness,
            make_json_http_request(
                "/v1/exec/start",
                Json{
                    {"cmd", tty_resize_command()},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 1000},
                }
            )
        );
        TEST_ASSERT(resize_start_response.status == 200);
        const Json resize_started = Json::parse(resize_start_response.body);
        TEST_ASSERT(resize_started.at("running").get<bool>());
        const std::string resize_session_id =
            resize_started.at("daemon_session_id").get<std::string>();
#ifdef _WIN32
        const Json resized =
            exec_write_json(harness, resize_session_id, "\r\n", 1000UL, true, 33U, 101U);
        TEST_ASSERT(resized.at("running").get<bool>());
#else
        std::string initial_size_output =
            normalize_output(resize_started.at("output").get<std::string>());
        if (!test_exec_pty::pty_size_output_matches(initial_size_output, 24U, 120U)) {
            initial_size_output = append_running_exec_output_until(
                harness,
                resize_session_id,
                initial_size_output,
                [](const std::string& value) {
                    return test_exec_pty::pty_size_output_matches(value, 24U, 120U);
                },
                2000UL
            );
        }
        TEST_ASSERT(test_exec_pty::pty_size_output_matches(initial_size_output, 24U, 120U));

        const Json resized =
            exec_write_json(harness, resize_session_id, "\n", 1000UL, true, 33U, 101U);
        TEST_ASSERT(resized.at("running").get<bool>());
        std::string resize_output = normalize_output(resized.at("output").get<std::string>());
        if (!test_exec_pty::pty_size_output_matches(resize_output, 33U, 101U)) {
            resize_output = append_running_exec_output_until(
                harness,
                resize_session_id,
                resize_output,
                [](const std::string& value) {
                    return test_exec_pty::pty_size_output_matches(value, 33U, 101U);
                },
                2000UL
            );
        }
        TEST_ASSERT(test_exec_pty::pty_size_output_matches(resize_output, 33U, 101U));
#endif
    }
}

int main(int argc, char** argv) {
#ifdef _WIN32
    if (argc >= 2 && std::string(argv[1]) == "--server-routes-helper") {
        return test_exec_pty::run_windows_stdin_helper(argc, argv, 2);
    }
#else
    (void)argc;
    (void)argv;
#endif
    const fs::path root = make_daemon_test_root("remote-exec-cpp-server-routes-test");
    TestRouteHarness harness(root);
    run_platform_neutral_server_route_tests(harness, root);
    assert_exec_routes(harness, root);

    return 0;
}
