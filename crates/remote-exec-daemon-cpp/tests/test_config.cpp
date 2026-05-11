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

static std::string minimal_config_text() {
    return "target = builder-cpp\n"
           "listen_host = 0.0.0.0\n"
           "listen_port = 8181\n"
           "default_workdir = C:\\work\n";
}

static bool config_rejected(const fs::path& path) {
    try {
        (void)load_config(path.string());
    } catch (...) {
        return true;
    }
    return false;
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
        "transfer_max_archive_bytes = 4096\n"
        "transfer_max_entry_bytes = 1024\n"
        "max_open_sessions = 12\n"
        "port_forward_max_worker_threads = 17\n"
        "port_forward_max_retained_sessions = 11\n"
        "port_forward_max_retained_listeners = 13\n"
        "port_forward_max_udp_binds = 15\n"
        "port_forward_max_active_tcp_streams = 19\n"
        "port_forward_max_tunnel_queued_bytes = 2097152\n"
        "port_forward_tunnel_io_timeout_ms = 7000\n"
        "port_forward_connect_timeout_ms = 8000\n"
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
    assert(config.transfer_limits.max_archive_bytes == 4096ULL);
    assert(config.transfer_limits.max_entry_bytes == 1024ULL);
    assert(config.max_open_sessions == 12UL);
    assert(config.port_forward_limits.max_worker_threads == 17UL);
    assert(config.port_forward_limits.max_retained_sessions == 11UL);
    assert(config.port_forward_limits.max_retained_listeners == 13UL);
    assert(config.port_forward_limits.max_udp_binds == 15UL);
    assert(config.port_forward_limits.max_active_tcp_streams == 19UL);
    assert(config.port_forward_limits.max_tunnel_queued_bytes == 2097152UL);
    assert(config.port_forward_limits.tunnel_io_timeout_ms == 7000UL);
    assert(config.port_forward_limits.connect_timeout_ms == 8000UL);
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
    assert(sandbox_config.port_forward_limits.max_worker_threads == DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
    assert(sandbox_config.port_forward_limits.max_retained_sessions == DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS);
    assert(sandbox_config.port_forward_limits.max_retained_listeners == DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS);
    assert(sandbox_config.port_forward_limits.max_udp_binds == DEFAULT_PORT_FORWARD_MAX_UDP_BINDS);
    assert(sandbox_config.port_forward_limits.max_active_tcp_streams == DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS);
    assert(sandbox_config.port_forward_limits.max_tunnel_queued_bytes == DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES);
    assert(sandbox_config.port_forward_limits.tunnel_io_timeout_ms == DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS);
    assert(sandbox_config.port_forward_limits.connect_timeout_ms == DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS);
    assert(sandbox_config.transfer_limits.max_archive_bytes == DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES);
    assert(sandbox_config.transfer_limits.max_entry_bytes == DEFAULT_TRANSFER_MAX_ENTRY_BYTES);
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
    assert(config_rejected(invalid_path));

    const fs::path invalid_worker_limit_path = root / "invalid-worker-limit.ini";
    write_text(
        invalid_worker_limit_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = C:\\work\n"
        "port_forward_max_worker_threads = 0\n"
    );
    assert(config_rejected(invalid_worker_limit_path));

    const fs::path invalid_transfer_zero_path = root / "invalid-transfer-zero.ini";
    write_text(
        invalid_transfer_zero_path,
        minimal_config_text() +
            "transfer_max_archive_bytes = 0\n"
            "transfer_max_entry_bytes = 1\n"
    );
    assert(config_rejected(invalid_transfer_zero_path));

    const fs::path invalid_transfer_bounds_path = root / "invalid-transfer-bounds.ini";
    write_text(
        invalid_transfer_bounds_path,
        minimal_config_text() +
            "transfer_max_archive_bytes = 8\n"
            "transfer_max_entry_bytes = 9\n"
    );
    assert(config_rejected(invalid_transfer_bounds_path));

    const char* invalid_limit_keys[] = {
        "port_forward_max_retained_sessions",
        "port_forward_max_retained_listeners",
        "port_forward_max_udp_binds",
        "port_forward_max_active_tcp_streams",
        "port_forward_max_tunnel_queued_bytes",
        "port_forward_tunnel_io_timeout_ms",
        "port_forward_connect_timeout_ms",
    };
    for (std::size_t index = 0;
         index < sizeof(invalid_limit_keys) / sizeof(invalid_limit_keys[0]);
         ++index) {
        const fs::path invalid_limit_path =
            root / ("invalid-" + std::string(invalid_limit_keys[index]) + ".ini");
        write_text(
            invalid_limit_path,
            minimal_config_text() + invalid_limit_keys[index] + " = 0\n"
        );
        assert(config_rejected(invalid_limit_path));
    }

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
    assert(config_rejected(invalid_yield_path));

    const fs::path invalid_auth_path = root / "invalid-auth.ini";
    write_text(
        invalid_auth_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = C:\\work\n"
        "http_auth_bearer_token = bad token\n"
    );
    assert(config_rejected(invalid_auth_path));

    const fs::path invalid_port_path = root / "invalid-port.ini";
    write_text(
        invalid_port_path,
        "target = builder-cpp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 70000\n"
        "default_workdir = C:\\work\n"
    );
    assert(config_rejected(invalid_port_path));

    return 0;
}
