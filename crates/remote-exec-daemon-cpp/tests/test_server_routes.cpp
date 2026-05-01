#include <cassert>
#include <cstdint>
#include <fstream>
#include <filesystem>
#include <iterator>
#include <string>

#include "config.h"
#include "http_helpers.h"
#include "platform.h"
#include "port_forward.h"
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

static void initialize_state(AppState& state, const fs::path& root) {
    state.config = make_config(root);
    state.daemon_instance_id = "test-instance";
    state.hostname = "test-host";
    state.default_shell = platform::resolve_default_shell("");
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

static void write_text_file(const fs::path& path, const std::string& value) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    output << value;
}

static std::string read_text_file(const fs::path& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

int main() {
    const fs::path root = make_test_root();
    AppState state;
    initialize_state(state, root);

    HttpRequest info_request;
    info_request.method = "POST";
    info_request.path = "/v1/target-info";
    const HttpResponse info_response = route_request(state, info_request);
    assert(info_response.status == 200);
    const Json info = Json::parse(info_response.body);
    assert(info.at("target").get<std::string>() == "cpp-test");
    assert(info.at("supports_pty").get<bool>() == process_session_supports_pty());
    assert(info.at("supports_port_forward").get<bool>());

    assert(normalize_port_forward_endpoint("8080") == "127.0.0.1:8080");
    assert(base64_decode_bytes(base64_encode_bytes(std::string("hello\0world", 11))).size() == 11);

    const HttpResponse listen_response = route_request(
        state,
        json_request(
            "/v1/port/listen",
            Json{{"endpoint", "127.0.0.1:0"}, {"protocol", "tcp"}}
        )
    );
    assert(listen_response.status == 200);
    const Json listen = Json::parse(listen_response.body);
    assert(listen.at("bind_id").get<std::string>().find("bind_") == 0);
    assert(listen.at("endpoint").get<std::string>().find("127.0.0.1:") == 0);

    const HttpResponse close_response = route_request(
        state,
        json_request(
            "/v1/port/listen/close",
            Json{{"bind_id", listen.at("bind_id").get<std::string>()}}
        )
    );
    assert(close_response.status == 200);

    const HttpResponse zero_connect_response = route_request(
        state,
        json_request(
            "/v1/port/connect",
            Json{{"endpoint", "127.0.0.1:0"}, {"protocol", "tcp"}}
        )
    );
    assert(zero_connect_response.status == 400);
    assert(Json::parse(zero_connect_response.body).at("code").get<std::string>() == "invalid_endpoint");

    const HttpResponse udp_listen_response = route_request(
        state,
        json_request(
            "/v1/port/listen",
            Json{{"endpoint", "127.0.0.1:0"}, {"protocol", "udp"}}
        )
    );
    assert(udp_listen_response.status == 200);
    const Json udp_listen = Json::parse(udp_listen_response.body);
    const HttpResponse udp_close_response = route_request(
        state,
        json_request(
            "/v1/port/listen/close",
            Json{{"bind_id", udp_listen.at("bind_id").get<std::string>()}}
        )
    );
    assert(udp_close_response.status == 200);

    const HttpResponse compression_response = route_request(
        state,
        json_request(
            "/v1/transfer/export",
            Json{{"path", (root / "missing.txt").string()}, {"compression", "zstd"}}
        )
    );
    assert(compression_response.status == 400);
    assert(
        Json::parse(compression_response.body).at("code").get<std::string>() ==
        "transfer_compression_unsupported"
    );

    const HttpResponse missing_source_response = route_request(
        state,
        json_request("/v1/transfer/export", Json{{"path", (root / "missing.txt").string()}})
    );
    assert(missing_source_response.status == 400);
    assert(
        Json::parse(missing_source_response.body).at("code").get<std::string>() ==
        "transfer_source_missing"
    );

    const fs::path source_file = root / "transfer-source.txt";
    write_text_file(source_file, "route transfer payload");

    const HttpResponse source_info_response = route_request(
        state,
        json_request("/v1/transfer/path-info", Json{{"path", source_file.string()}})
    );
    assert(source_info_response.status == 200);
    const Json source_info = Json::parse(source_info_response.body);
    assert(source_info.at("exists").get<bool>());
    assert(!source_info.at("is_directory").get<bool>());

    const HttpResponse root_info_response = route_request(
        state,
        json_request("/v1/transfer/path-info", Json{{"path", root.string()}})
    );
    assert(root_info_response.status == 200);
    const Json root_info = Json::parse(root_info_response.body);
    assert(root_info.at("exists").get<bool>());
    assert(root_info.at("is_directory").get<bool>());

    const HttpResponse export_response = route_request(
        state,
        json_request("/v1/transfer/export", Json{{"path", source_file.string()}})
    );
    assert(export_response.status == 200);
    assert(export_response.headers.at("Content-Type") == "application/octet-stream");
    assert(export_response.headers.at("x-remote-exec-source-type") == "file");
    assert(export_response.headers.at("x-remote-exec-compression") == "none");
    assert(!export_response.body.empty());

    HttpRequest import_request;
    import_request.method = "POST";
    import_request.path = "/v1/transfer/import";
    import_request.headers["x-remote-exec-source-type"] = "file";
    import_request.headers["x-remote-exec-destination-path"] = (root / "transfer-dest.txt").string();
    import_request.headers["x-remote-exec-overwrite"] = "replace";
    import_request.headers["x-remote-exec-create-parent"] = "true";
    import_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    import_request.headers["x-remote-exec-compression"] = "none";
    import_request.body = export_response.body;

    const HttpResponse import_response = route_request(state, import_request);
    assert(import_response.status == 200);
    const Json imported = Json::parse(import_response.body);
    assert(imported.at("source_type").get<std::string>() == "file");
    assert(imported.at("files_copied").get<std::uint64_t>() == 1);
    assert(imported.at("bytes_copied").get<std::uint64_t>() == 22);
    assert(imported.at("replaced").get<bool>() == false);
    assert(imported.at("warnings").empty());
    assert(read_text_file(root / "transfer-dest.txt") == "route transfer payload");

#ifndef _WIN32
    const HttpResponse non_tty_start_response = route_request(
        state,
        json_request(
            "/v1/exec/start",
            Json{
                {"cmd", "printf ready; sleep 5"},
                {"workdir", root.string()},
                {"login", false},
                {"tty", false},
                {"yield_time_ms", 250},
            }
        )
    );
    assert(non_tty_start_response.status == 200);
    const Json non_tty_started = Json::parse(non_tty_start_response.body);
    assert(non_tty_started.at("running").get<bool>());
    assert(non_tty_started.at("output").get<std::string>() == "ready");

    const HttpResponse stdin_closed_response = route_request(
        state,
        json_request(
            "/v1/exec/write",
            Json{
                {"daemon_session_id", non_tty_started.at("daemon_session_id").get<std::string>()},
                {"chars", "hello\n"},
                {"yield_time_ms", 250},
            }
        )
    );
    assert(stdin_closed_response.status == 400);
    assert(
        Json::parse(stdin_closed_response.body).at("code").get<std::string>() ==
        "stdin_closed"
    );

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
