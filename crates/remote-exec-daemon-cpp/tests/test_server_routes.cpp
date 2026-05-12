#include <cassert>
#include <cstdint>
#include <string>
#include <thread>

#include "http_helpers.h"
#include "platform.h"
#include "process_session.h"
#include "server_routes.h"
#include "test_server_routes_shared.h"

namespace fs = test_fs;

static HttpRequest json_request(const std::string& path, const Json& body) {
    HttpRequest request;
    request.method = "POST";
    request.path = path;
    request.headers["content-type"] = "application/json";
    request.body = body.dump();
    return request;
}

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

static void assert_exec_routes(AppState& state, const fs::path& root) {
    const HttpResponse non_tty_start_response = route_request(state,
                                                              json_request("/v1/exec/start",
                                                                           Json{
                                                                               {"cmd", "printf ready; sleep 5"},
                                                                               {"workdir", root.string()},
                                                                               {"login", false},
                                                                               {"tty", false},
                                                                               {"yield_time_ms", 250},
                                                                           }));
    assert(non_tty_start_response.status == 200);
    const Json non_tty_started = Json::parse(non_tty_start_response.body);
    assert(non_tty_started.at("running").get<bool>());
    assert(non_tty_started.at("output").get<std::string>() == "ready");

    const HttpResponse stdin_closed_response = route_request(
        state,
        json_request("/v1/exec/write",
                     Json{
                         {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                         {"chars", "hello\n"},
                         {"yield_time_ms", 250},
                     }));
    assert(stdin_closed_response.status == 400);
    assert(Json::parse(stdin_closed_response.body).at("code").get<std::string>() == "stdin_closed");

    const HttpResponse invalid_pty_size_response = route_request(
        state,
        json_request("/v1/exec/write",
                     Json{
                         {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                         {"chars", ""},
                         {"yield_time_ms", 250},
                         {"pty_size", Json{{"rows", 0}, {"cols", 80}}},
                     }));
    assert(invalid_pty_size_response.status == 400);
    assert(Json::parse(invalid_pty_size_response.body).at("code").get<std::string>() == "invalid_pty_size");

    const HttpResponse non_tty_resize_response = route_request(
        state,
        json_request("/v1/exec/write",
                     Json{
                         {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                         {"chars", ""},
                         {"yield_time_ms", 250},
                         {"pty_size", Json{{"rows", 33}, {"cols", 101}}},
                     }));
    assert(non_tty_resize_response.status == 400);
    assert(Json::parse(non_tty_resize_response.body).at("code").get<std::string>() == "tty_unsupported");

    if (process_session_supports_pty()) {
        const HttpResponse slow_start_response = route_request(state,
                                                               json_request("/v1/exec/start",
                                                                            Json{
                                                                                {"cmd", "printf slow; sleep 30"},
                                                                                {"workdir", root.string()},
                                                                                {"login", false},
                                                                                {"tty", true},
                                                                                {"yield_time_ms", 250},
                                                                            }));
        assert(slow_start_response.status == 200);
        const Json slow_started = Json::parse(slow_start_response.body);
        assert(slow_started.at("running").get<bool>());

        const HttpResponse fast_start_response =
            route_request(state,
                          json_request("/v1/exec/start",
                                       Json{
                                           {"cmd", "IFS= read line; printf '%s' \"$line\"; sleep 30"},
                                           {"workdir", root.string()},
                                           {"login", false},
                                           {"tty", true},
                                           {"yield_time_ms", 250},
                                       }));
        assert(fast_start_response.status == 200);
        const Json fast_started = Json::parse(fast_start_response.body);
        assert(fast_started.at("running").get<bool>());

        HttpResponse slow_poll_response;
        std::thread slow_thread([&]() {
            slow_poll_response = route_request(
                state,
                json_request("/v1/exec/write",
                             Json{
                                 {"daemon_session_id", slow_started.at("daemon_session_id").get<std::string>()},
                                 {"chars", ""},
                                 {"yield_time_ms", 5000},
                             }));
        });

        platform::sleep_ms(200);
        const std::uint64_t fast_started_at = platform::monotonic_ms();
        const HttpResponse fast_write_response = route_request(
            state,
            json_request("/v1/exec/write",
                         Json{
                             {"daemon_session_id", fast_started.at("daemon_session_id").get<std::string>()},
                             {"chars", "ping\n"},
                             {"yield_time_ms", 250},
                         }));
        const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
        assert(fast_write_response.status == 200);
        assert(fast_elapsed_ms < 2000UL && "fast route request waited behind unrelated session");
        assert(Json::parse(fast_write_response.body).at("output").get<std::string>().find("ping") != std::string::npos);
        slow_thread.join();
        assert(slow_poll_response.status == 200);

        const HttpResponse start_response =
            route_request(state,
                          json_request("/v1/exec/start",
                                       Json{
                                           {"cmd",
                                            "if test -t 0; then printf 'tty:yes\\n'; else printf 'tty:no\\n'; fi; "
                                            "IFS= read line; printf 'input:%s\\n' \"$line\""},
                                           {"workdir", root.string()},
                                           {"login", false},
                                           {"tty", true},
                                           {"yield_time_ms", 250},
                                       }));
        assert(start_response.status == 200);
        const Json started = Json::parse(start_response.body);
        assert(started.at("running").get<bool>());
        assert(normalize_output(started.at("output").get<std::string>()) == "tty:yes\n");

        const HttpResponse write_response =
            route_request(state,
                          json_request("/v1/exec/write",
                                       Json{
                                           {"daemon_session_id", started.at("daemon_session_id").get<std::string>()},
                                           {"chars", "hello\n"},
                                           {"yield_time_ms", 5000},
                                       }));
        assert(write_response.status == 200);
        const Json completed = Json::parse(write_response.body);
        assert(!completed.at("running").get<bool>());
        assert(completed.at("exit_code").get<int>() == 0);
        const std::string output = normalize_output(completed.at("output").get<std::string>());
        assert(output.find("hello\n") != std::string::npos);
        assert(output.find("input:hello\n") != std::string::npos);

        const HttpResponse resize_start_response =
            route_request(state,
                          json_request("/v1/exec/start",
                                       Json{
                                           {"cmd", "printf ready; IFS= read line; stty size; sleep 30"},
                                           {"workdir", root.string()},
                                           {"login", false},
                                           {"tty", true},
                                           {"yield_time_ms", 250},
                                       }));
        assert(resize_start_response.status == 200);
        const Json resize_started = Json::parse(resize_start_response.body);
        assert(resize_started.at("running").get<bool>());

        const HttpResponse resize_write_response = route_request(
            state,
            json_request("/v1/exec/write",
                         Json{
                             {"daemon_session_id", resize_started.at("daemon_session_id").get<std::string>()},
                             {"chars", "\n"},
                             {"yield_time_ms", 1000},
                             {"pty_size", Json{{"rows", 33}, {"cols", 101}}},
                         }));
        assert(resize_write_response.status == 200);
        const Json resized = Json::parse(resize_write_response.body);
        assert(resized.at("running").get<bool>());
        assert(normalize_output(resized.at("output").get<std::string>()).find("33 101") != std::string::npos);
    }
}

int main() {
    const fs::path root = make_server_routes_test_root("remote-exec-cpp-server-routes-test");
    AppState state;
    initialize_server_routes_state(state, root);

    run_platform_neutral_server_route_tests(state, root);
    assert_exec_routes(state, root);

    return 0;
}
