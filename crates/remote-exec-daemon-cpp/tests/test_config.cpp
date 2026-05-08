#include <cassert>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>

#include "config.h"

namespace fs = std::filesystem;

static void write_text(const fs::path& path, const std::string& value) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    output << value;
}

int main() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-config-test";
    fs::remove_all(root);
    fs::create_directories(root);

    const fs::path config_path = root / "daemon-cpp.ini";
    write_text(
        config_path,
        "# comment\n"
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = \"C:\\work dir\"\n"
        "default_shell = /bin/sh\n"
        "allow_login_shell = false\n"
        "http_auth_bearer_token = shared-secret\n"
        "max_request_header_bytes = 32768\n"
        "max_request_body_bytes = 1048576\n"
        "max_open_sessions = 12\n"
        "port_forward_max_worker_threads = 17\n"
        "yield_time_exec_command_default_ms = 15000\n"
        "yield_time_exec_command_max_ms = 60000\n"
        "yield_time_exec_command_min_ms = 500\n"
        "yield_time_write_stdin_poll_default_ms = 12000\n"
        "yield_time_write_stdin_input_max_ms = 45000\n"
    );

    const DaemonConfig config = load_config(config_path.string());
    assert(config.target == "builder-cpp");
    assert(config.listen_host == "0.0.0.0");
    assert(config.listen_port == 8181);
    assert(config.default_workdir == "C:\\work dir");
    assert(config.default_shell == "/bin/sh");
    assert(!config.allow_login_shell);
    assert(config.http_auth_bearer_token == "shared-secret");
    assert(config.max_request_header_bytes == 32768UL);
    assert(config.max_request_body_bytes == 1048576UL);
    assert(config.max_open_sessions == 12UL);
    assert(config.port_forward_max_worker_threads == 17UL);
    assert(config.yield_time.exec_command.default_ms == 15000UL);
    assert(config.yield_time.exec_command.max_ms == 60000UL);
    assert(config.yield_time.exec_command.min_ms == 500UL);
    assert(config.yield_time.write_stdin_poll.default_ms == 12000UL);
    assert(config.yield_time.write_stdin_poll.max_ms == 300000UL);
    assert(config.yield_time.write_stdin_poll.min_ms == 5000UL);
    assert(config.yield_time.write_stdin_input.default_ms == 250UL);
    assert(config.yield_time.write_stdin_input.max_ms == 45000UL);
    assert(config.yield_time.write_stdin_input.min_ms == 250UL);
    assert(!config.sandbox_configured);

    const fs::path sandbox_config_path = root / "sandbox.ini";
    write_text(
        sandbox_config_path,
        "target = sandbox-cpp\n"
        "listen_host = 127.0.0.1\n"
        "listen_port = 8181\n"
        "default_workdir = /work\n"
        "sandbox_exec_cwd_allow = /work;/tmp/work\n"
        "sandbox_exec_cwd_deny = /work/private\n"
        "sandbox_read_allow = /work;/assets\n"
        "sandbox_read_deny = /work/.git;/assets/secrets\n"
        "sandbox_write_allow = /work\n"
        "sandbox_write_deny = /work/.git;/work/readonly\n"
    );
    const DaemonConfig sandbox_config = load_config(sandbox_config_path.string());
    assert(sandbox_config.port_forward_max_worker_threads == DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
    assert(sandbox_config.sandbox_configured);
    assert(sandbox_config.sandbox.exec_cwd.allow.size() == 2);
    assert(sandbox_config.sandbox.exec_cwd.allow[0] == "/work");
    assert(sandbox_config.sandbox.exec_cwd.allow[1] == "/tmp/work");
    assert(sandbox_config.sandbox.exec_cwd.deny[0] == "/work/private");
    assert(sandbox_config.sandbox.read.allow[1] == "/assets");
    assert(sandbox_config.sandbox.read.deny[1] == "/assets/secrets");
    assert(sandbox_config.sandbox.write.allow[0] == "/work");
    assert(sandbox_config.sandbox.write.deny[1] == "/work/readonly");

    const YieldTimeConfig defaults = default_yield_time_config();
    assert(resolve_yield_time_ms(defaults.exec_command, false, 0UL) == 10000UL);
    assert(resolve_yield_time_ms(defaults.exec_command, true, 1UL) == 250UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_poll, false, 0UL) == 5000UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_poll, true, 1UL) == 5000UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_poll, true, 400000UL) == 300000UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_input, false, 0UL) == 250UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_input, true, 50000UL) == 30000UL);

    const fs::path invalid_path = root / "invalid.ini";
    write_text(invalid_path, "target builder-cpp\n");
    bool rejected = false;
    try {
        (void)load_config(invalid_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    const fs::path invalid_worker_limit_path = root / "invalid-worker-limit.ini";
    write_text(
        invalid_worker_limit_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = C:\\work\n"
        "port_forward_max_worker_threads = 0\n"
    );
    rejected = false;
    try {
        (void)load_config(invalid_worker_limit_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    const fs::path invalid_yield_path = root / "invalid-yield.ini";
    write_text(
        invalid_yield_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = C:\\work\n"
        "yield_time_exec_command_default_ms = 10\n"
        "yield_time_exec_command_min_ms = 20\n"
    );
    rejected = false;
    try {
        (void)load_config(invalid_yield_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    const fs::path invalid_auth_path = root / "invalid-auth.ini";
    write_text(
        invalid_auth_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = C:\\work\n"
        "http_auth_bearer_token = bad token\n"
    );
    rejected = false;
    try {
        (void)load_config(invalid_auth_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    const fs::path invalid_port_path = root / "invalid-port.ini";
    write_text(
        invalid_port_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 70000\n"
        "default_workdir = C:\\work\n"
    );
    rejected = false;
    try {
        (void)load_config(invalid_port_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    return 0;
}
