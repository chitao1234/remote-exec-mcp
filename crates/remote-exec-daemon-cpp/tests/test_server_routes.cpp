#include <cassert>
#include <filesystem>
#include <string>

#include "config.h"
#include "http_helpers.h"
#include "platform.h"
#include "process_session.h"
#include "server_routes.h"

namespace fs = std::filesystem;

static fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-server-routes-test";
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

static DaemonConfig make_config(const fs::path& root) {
    DaemonConfig config;
    config.target = "cpp-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = root.string();
    config.default_shell.clear();
    config.allow_login_shell = true;
    config.http_auth_bearer_token.clear();
    config.max_request_header_bytes = 65536;
    config.max_request_body_bytes = 536870912;
    config.max_open_sessions = 64;
    config.yield_time = default_yield_time_config();
    return config;
}

static AppState make_state(const fs::path& root) {
    AppState state;
    state.config = make_config(root);
    state.daemon_instance_id = "test-instance";
    state.hostname = "test-host";
    state.default_shell = platform::resolve_default_shell("");
    return state;
}

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

int main() {
    const fs::path root = make_test_root();
    AppState state = make_state(root);

    HttpRequest info_request;
    info_request.method = "POST";
    info_request.path = "/v1/target-info";
    const HttpResponse info_response = route_request(state, info_request);
    assert(info_response.status == 200);
    const Json info = Json::parse(info_response.body);
    assert(info.at("target").get<std::string>() == "cpp-test");
    assert(info.at("supports_pty").get<bool>() == process_session_supports_pty());

#ifndef _WIN32
    if (process_session_supports_pty()) {
        const HttpResponse start_response = route_request(
            state,
            json_request(
                "/v1/exec/start",
                Json{
                    {"cmd",
                     "if test -t 0; then printf 'tty:yes\\n'; else printf 'tty:no\\n'; fi; "
                     "IFS= read line; printf 'input:%s\\n' \"$line\""},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        );
        assert(start_response.status == 200);
        const Json started = Json::parse(start_response.body);
        assert(started.at("running").get<bool>());
        assert(normalize_output(started.at("output").get<std::string>()) == "tty:yes\n");

        const HttpResponse write_response = route_request(
            state,
            json_request(
                "/v1/exec/write",
                Json{
                    {"daemon_session_id", started.at("daemon_session_id").get<std::string>()},
                    {"chars", "hello\n"},
                    {"yield_time_ms", 5000},
                }
            )
        );
        assert(write_response.status == 200);
        const Json completed = Json::parse(write_response.body);
        assert(!completed.at("running").get<bool>());
        assert(completed.at("exit_code").get<int>() == 0);
        const std::string output = normalize_output(completed.at("output").get<std::string>());
        assert(output.find("hello\n") != std::string::npos);
        assert(output.find("input:hello\n") != std::string::npos);
    }
#endif

    return 0;
}
