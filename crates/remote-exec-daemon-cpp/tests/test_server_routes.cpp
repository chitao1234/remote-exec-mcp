#include <cassert>
#include <cstdint>
#include <fstream>
#include <filesystem>
#include <iterator>
#include <string>
#include <thread>

#include "config.h"
#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "path_policy.h"
#include "platform.h"
#include "port_forward.h"
#include "process_session.h"
#include "server_routes.h"
#include "transfer_ops.h"

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

static void enable_sandbox(AppState& state) {
    state.sandbox_enabled = state.config.sandbox_configured;
    if (state.sandbox_enabled) {
        state.sandbox = compile_filesystem_sandbox(host_path_policy(), state.config.sandbox);
    }
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

static std::string read_binary_file(const fs::path& path) {
    return read_text_file(path);
}

static void write_binary_file(const fs::path& path, const std::string& value) {
    write_text_file(path, value);
}

static std::string decode_data_url_bytes(const std::string& image_url) {
    const std::size_t comma = image_url.find(',');
    assert(comma != std::string::npos);
    return base64_decode_bytes(image_url.substr(comma + 1));
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
    assert(info.at("supports_image_read").get<bool>());
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

    const fs::path image_file = root / "tiny.png";
    write_binary_file(
        image_file,
        base64_decode_bytes(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aL9sAAAAASUVORK5CYII="
        )
    );
    const std::string original_image = read_binary_file(image_file);

    const HttpResponse image_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "tiny.png"}, {"workdir", root.string()}}
        )
    );
    assert(image_response.status == 200);
    const Json image = Json::parse(image_response.body);
    assert(image.at("detail").get<std::string>() == "original");
    assert(image.at("image_url").get<std::string>().find("data:image/png;base64,") == 0);
    assert(decode_data_url_bytes(image.at("image_url").get<std::string>()) == original_image);

    const HttpResponse invalid_detail_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "tiny.png"}, {"workdir", root.string()}, {"detail", "low"}}
        )
    );
    assert(invalid_detail_response.status == 400);
    const Json invalid_detail = Json::parse(invalid_detail_response.body);
    assert(invalid_detail.at("code").get<std::string>() == "invalid_detail");

    const HttpResponse missing_image_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "missing.png"}, {"workdir", root.string()}}
        )
    );
    assert(missing_image_response.status == 400);
    const Json missing_image = Json::parse(missing_image_response.body);
    assert(missing_image.at("code").get<std::string>() == "image_missing");

    const fs::path gif_file = root / "tiny.gif";
    write_binary_file(
        gif_file,
        base64_decode_bytes("R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==")
    );

    const HttpResponse gif_response = route_request(
        state,
        json_request(
            "/v1/image/read",
            Json{{"path", "tiny.gif"}, {"workdir", root.string()}}
        )
    );
    assert(gif_response.status == 400);
    const Json gif_error = Json::parse(gif_response.body);
    assert(gif_error.at("code").get<std::string>() == "image_decode_failed");

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

    const HttpResponse relative_info_response = route_request(
        state,
        json_request("/v1/transfer/path-info", Json{{"path", "relative/path.txt"}})
    );
    assert(relative_info_response.status == 400);
    const Json relative_info_error = Json::parse(relative_info_response.body);
    assert(relative_info_error.at("code").get<std::string>() == "transfer_path_not_absolute");

    const HttpResponse export_response = route_request(
        state,
        json_request("/v1/transfer/export", Json{{"path", source_file.string()}})
    );
    assert(export_response.status == 200);
    assert(export_response.headers.at("Content-Type") == "application/octet-stream");
    assert(export_response.headers.at("x-remote-exec-source-type") == "file");
    assert(export_response.headers.at("x-remote-exec-compression") == "none");
    assert(!export_response.body.empty());

    const fs::path exclude_source = root / "transfer-exclude-source";
    fs::create_directories(exclude_source / ".git");
    fs::create_directories(exclude_source / "logs");
    write_text_file(exclude_source / "keep.txt", "keep");
    write_text_file(exclude_source / "top.log", "drop");
    write_text_file(exclude_source / ".git" / "config", "secret");
    write_text_file(exclude_source / "logs" / "readme.txt", "keep");
    write_text_file(exclude_source / "logs" / "app.log", "drop");
    Json exclude_patterns = Json::array();
    exclude_patterns.push_back("**/*.log");
    exclude_patterns.push_back(".git/**");
    const HttpResponse export_excluded_response = route_request(
        state,
        json_request(
            "/v1/transfer/export",
            Json{{"path", exclude_source.string()}, {"exclude", exclude_patterns}}
        )
    );
    assert(export_excluded_response.status == 200);
    const ImportSummary excluded_import = import_path(
        export_excluded_response.body,
        "directory",
        (root / "transfer-exclude-dest").string(),
        "replace",
        true
    );
    assert(excluded_import.warnings.empty());
    assert(read_text_file(root / "transfer-exclude-dest" / "keep.txt") == "keep");
    assert(read_text_file(root / "transfer-exclude-dest" / "logs" / "readme.txt") == "keep");
    assert(!fs::exists(root / "transfer-exclude-dest" / "top.log"));
    assert(!fs::exists(root / "transfer-exclude-dest" / ".git"));
    assert(!fs::exists(root / "transfer-exclude-dest" / "logs" / "app.log"));

    Json malformed_exclude = Json::array();
    malformed_exclude.push_back("tmp/[abc");
    const HttpResponse invalid_exclude_response = route_request(
        state,
        json_request(
            "/v1/transfer/export",
            Json{{"path", exclude_source.string()}, {"exclude", malformed_exclude}}
        )
    );
    assert(invalid_exclude_response.status == 400);
    const Json invalid_exclude = Json::parse(invalid_exclude_response.body);
    assert(invalid_exclude.at("code").get<std::string>() == "transfer_failed");
    assert(
        invalid_exclude.at("message").get<std::string>().find("invalid exclude pattern") !=
        std::string::npos
    );

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

    fs::create_directories(root / "merge-dir");
    HttpRequest merge_file_into_directory_request;
    merge_file_into_directory_request.method = "POST";
    merge_file_into_directory_request.path = "/v1/transfer/import";
    merge_file_into_directory_request.headers["x-remote-exec-source-type"] = "file";
    merge_file_into_directory_request.headers["x-remote-exec-destination-path"] =
        (root / "merge-dir").string();
    merge_file_into_directory_request.headers["x-remote-exec-overwrite"] = "merge";
    merge_file_into_directory_request.headers["x-remote-exec-create-parent"] = "true";
    merge_file_into_directory_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    merge_file_into_directory_request.headers["x-remote-exec-compression"] = "none";
    merge_file_into_directory_request.body = export_response.body;
    const HttpResponse merge_file_into_directory_response =
        route_request(state, merge_file_into_directory_request);
    assert(merge_file_into_directory_response.status == 400);
    const Json merge_file_into_directory_error =
        Json::parse(merge_file_into_directory_response.body);
    assert(
        merge_file_into_directory_error.at("code").get<std::string>() ==
        "transfer_destination_unsupported"
    );

    const fs::path sandbox_root = root / "sandbox";
    const fs::path exec_allowed = sandbox_root / "exec";
    const fs::path read_allowed = sandbox_root / "read";
    const fs::path write_allowed = sandbox_root / "write";
    const fs::path outside = sandbox_root / "outside";
    fs::create_directories(exec_allowed);
    fs::create_directories(read_allowed);
    fs::create_directories(write_allowed);
    fs::create_directories(outside);
    write_text_file(read_allowed / "source.txt", "sandbox source");
    write_text_file(outside / "outside.txt", "outside");

    AppState sandbox_state;
    initialize_state(sandbox_state, root);
    sandbox_state.config.sandbox_configured = true;
    sandbox_state.config.sandbox.exec_cwd.allow.push_back(exec_allowed.string());
    sandbox_state.config.sandbox.read.allow.push_back(read_allowed.string());
    sandbox_state.config.sandbox.write.allow.push_back(write_allowed.string());
    enable_sandbox(sandbox_state);

    const HttpResponse sandbox_export_denied = route_request(
        sandbox_state,
        json_request("/v1/transfer/export", Json{{"path", (outside / "outside.txt").string()}})
    );
    assert(sandbox_export_denied.status == 400);
    assert(
        Json::parse(sandbox_export_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

    const HttpResponse sandbox_path_info_denied = route_request(
        sandbox_state,
        json_request("/v1/transfer/path-info", Json{{"path", (outside / "dest.txt").string()}})
    );
    assert(sandbox_path_info_denied.status == 400);
    assert(
        Json::parse(sandbox_path_info_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

    const HttpResponse sandbox_export_allowed = route_request(
        sandbox_state,
        json_request("/v1/transfer/export", Json{{"path", (read_allowed / "source.txt").string()}})
    );
    assert(sandbox_export_allowed.status == 200);

    HttpRequest sandbox_import_denied_request;
    sandbox_import_denied_request.method = "POST";
    sandbox_import_denied_request.path = "/v1/transfer/import";
    sandbox_import_denied_request.headers["x-remote-exec-source-type"] = "file";
    sandbox_import_denied_request.headers["x-remote-exec-destination-path"] =
        (outside / "dest.txt").string();
    sandbox_import_denied_request.headers["x-remote-exec-overwrite"] = "replace";
    sandbox_import_denied_request.headers["x-remote-exec-create-parent"] = "true";
    sandbox_import_denied_request.headers["x-remote-exec-symlink-mode"] = "preserve";
    sandbox_import_denied_request.headers["x-remote-exec-compression"] = "none";
    sandbox_import_denied_request.body = sandbox_export_allowed.body;
    const HttpResponse sandbox_import_denied =
        route_request(sandbox_state, sandbox_import_denied_request);
    assert(sandbox_import_denied.status == 400);
    assert(
        Json::parse(sandbox_import_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

    const std::string patch_denied_text =
        "*** Begin Patch\n"
        "*** Add File: " + (outside / "patched.txt").string() + "\n"
        "+denied\n"
        "*** End Patch\n";
    const HttpResponse sandbox_patch_denied = route_request(
        sandbox_state,
        json_request("/v1/patch/apply", Json{{"workdir", write_allowed.string()}, {"patch", patch_denied_text}})
    );
    assert(sandbox_patch_denied.status == 400);
    assert(
        Json::parse(sandbox_patch_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );
    assert(!fs::exists(outside / "patched.txt"));

    const HttpResponse sandbox_exec_denied = route_request(
        sandbox_state,
        json_request(
            "/v1/exec/start",
            Json{
                {"cmd", "printf denied"},
                {"workdir", outside.string()},
                {"login", false},
                {"tty", false},
                {"yield_time_ms", 250},
            }
        )
    );
    assert(sandbox_exec_denied.status == 400);
    assert(
        Json::parse(sandbox_exec_denied.body).at("code").get<std::string>() ==
        "sandbox_denied"
    );

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
        const HttpResponse slow_start_response = route_request(
            state,
            json_request(
                "/v1/exec/start",
                Json{
                    {"cmd", "printf slow; sleep 30"},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        );
        assert(slow_start_response.status == 200);
        const Json slow_started = Json::parse(slow_start_response.body);
        assert(slow_started.at("running").get<bool>());

        const HttpResponse fast_start_response = route_request(
            state,
            json_request(
                "/v1/exec/start",
                Json{
                    {"cmd", "IFS= read line; printf '%s' \"$line\"; sleep 30"},
                    {"workdir", root.string()},
                    {"login", false},
                    {"tty", true},
                    {"yield_time_ms", 250},
                }
            )
        );
        assert(fast_start_response.status == 200);
        const Json fast_started = Json::parse(fast_start_response.body);
        assert(fast_started.at("running").get<bool>());

        HttpResponse slow_poll_response;
        std::thread slow_thread([&]() {
            slow_poll_response = route_request(
                state,
                json_request(
                    "/v1/exec/write",
                    Json{
                        {"daemon_session_id", slow_started.at("daemon_session_id").get<std::string>()},
                        {"chars", ""},
                        {"yield_time_ms", 5000},
                    }
                )
            );
        });

        platform::sleep_ms(200);
        const std::uint64_t fast_started_at = platform::monotonic_ms();
        const HttpResponse fast_write_response = route_request(
            state,
            json_request(
                "/v1/exec/write",
                Json{
                    {"daemon_session_id", fast_started.at("daemon_session_id").get<std::string>()},
                    {"chars", "ping\n"},
                    {"yield_time_ms", 250},
                }
            )
        );
        const std::uint64_t fast_elapsed_ms = platform::monotonic_ms() - fast_started_at;
        assert(fast_write_response.status == 200);
        assert(
            fast_elapsed_ms < 2000UL &&
            "fast route request waited behind unrelated session"
        );
        assert(
            Json::parse(fast_write_response.body)
                .at("output")
                .get<std::string>()
                .find("ping") != std::string::npos
        );
        slow_thread.join();
        assert(slow_poll_response.status == 200);

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
