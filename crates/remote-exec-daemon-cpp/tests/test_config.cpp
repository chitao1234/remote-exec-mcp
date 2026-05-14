#include "test_assert.h"
#include <cstdio>
#include <string>

#include "config.h"
#include "test_filesystem.h"

namespace fs = test_fs;

static void write_text(const fs::path& path, const std::string& value) {
    fs::write_file_bytes(path, value);
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
    const DaemonConfig default_config = DaemonConfig();
    TEST_ASSERT(default_config.max_request_header_bytes == DEFAULT_MAX_REQUEST_HEADER_BYTES);
    TEST_ASSERT(default_config.max_request_body_bytes == DEFAULT_MAX_REQUEST_BODY_BYTES);
    TEST_ASSERT(default_config.max_open_sessions == DEFAULT_MAX_OPEN_SESSIONS);
    TEST_ASSERT(default_config.http_connection_idle_timeout_ms == DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS);
    TEST_ASSERT(default_config.port_forward_limits.max_worker_threads == DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
    TEST_ASSERT(default_config.port_forward_limits.max_retained_sessions == DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS);
    TEST_ASSERT(default_config.port_forward_limits.max_retained_listeners == DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS);
    TEST_ASSERT(default_config.port_forward_limits.max_udp_binds == DEFAULT_PORT_FORWARD_MAX_UDP_BINDS);
    TEST_ASSERT(default_config.port_forward_limits.max_active_tcp_streams == DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS);
    TEST_ASSERT(default_config.port_forward_limits.max_tunnel_queued_bytes == DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES);
    TEST_ASSERT(default_config.port_forward_limits.tunnel_io_timeout_ms == DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS);
    TEST_ASSERT(default_config.port_forward_limits.connect_timeout_ms == DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS);
    TEST_ASSERT(default_config.yield_time.exec_command.default_ms == DEFAULT_YIELD_TIME_EXEC_COMMAND_DEFAULT_MS);
    TEST_ASSERT(default_config.yield_time.exec_command.max_ms == DEFAULT_YIELD_TIME_EXEC_COMMAND_MAX_MS);
    TEST_ASSERT(default_config.yield_time.exec_command.min_ms == DEFAULT_YIELD_TIME_EXEC_COMMAND_MIN_MS);
    TEST_ASSERT(default_config.yield_time.write_stdin_poll.default_ms == DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_DEFAULT_MS);
    TEST_ASSERT(default_config.yield_time.write_stdin_poll.max_ms == DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MAX_MS);
    TEST_ASSERT(default_config.yield_time.write_stdin_poll.min_ms == DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MIN_MS);
    TEST_ASSERT(default_config.yield_time.write_stdin_input.default_ms == DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_DEFAULT_MS);
    TEST_ASSERT(default_config.yield_time.write_stdin_input.max_ms == DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MAX_MS);
    TEST_ASSERT(default_config.yield_time.write_stdin_input.min_ms == DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MIN_MS);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-config-test";
    fs::remove_all(root);
    fs::create_directories(root);

    const fs::path config_path = root / "daemon-cpp.ini";
    write_text(config_path,
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
               "http_connection_idle_timeout_ms = 9000\n"
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
               "yield_time_write_stdin_input_max_ms = 45000\n");

    const DaemonConfig config = load_config(config_path.string());
    TEST_ASSERT(config.target == "builder-cpp");
    TEST_ASSERT(config.listen_host == "0.0.0.0");
    TEST_ASSERT(config.listen_port == 8181);
    TEST_ASSERT(config.default_workdir == "C:\\work dir");
    TEST_ASSERT(config.default_shell == "/bin/sh");
    TEST_ASSERT(!config.allow_login_shell);
    TEST_ASSERT(config.http_auth_bearer_token == "shared-secret");
    TEST_ASSERT(config.max_request_header_bytes == 32768UL);
    TEST_ASSERT(config.max_request_body_bytes == 1048576UL);
    TEST_ASSERT(config.http_connection_idle_timeout_ms == 9000UL);
    TEST_ASSERT(config.transfer_limits.max_archive_bytes == 4096ULL);
    TEST_ASSERT(config.transfer_limits.max_entry_bytes == 1024ULL);
    TEST_ASSERT(config.max_open_sessions == 12UL);
    TEST_ASSERT(config.port_forward_limits.max_worker_threads == 17UL);
    TEST_ASSERT(config.port_forward_limits.max_retained_sessions == 11UL);
    TEST_ASSERT(config.port_forward_limits.max_retained_listeners == 13UL);
    TEST_ASSERT(config.port_forward_limits.max_udp_binds == 15UL);
    TEST_ASSERT(config.port_forward_limits.max_active_tcp_streams == 19UL);
    TEST_ASSERT(config.port_forward_limits.max_tunnel_queued_bytes == 2097152UL);
    TEST_ASSERT(config.port_forward_limits.tunnel_io_timeout_ms == 7000UL);
    TEST_ASSERT(config.port_forward_limits.connect_timeout_ms == 8000UL);
    TEST_ASSERT(config.yield_time.exec_command.default_ms == 15000UL);
    TEST_ASSERT(config.yield_time.exec_command.max_ms == 60000UL);
    TEST_ASSERT(config.yield_time.exec_command.min_ms == 500UL);
    TEST_ASSERT(config.yield_time.write_stdin_poll.default_ms == 12000UL);
    TEST_ASSERT(config.yield_time.write_stdin_poll.max_ms == 300000UL);
    TEST_ASSERT(config.yield_time.write_stdin_poll.min_ms == 5000UL);
    TEST_ASSERT(config.yield_time.write_stdin_input.default_ms == 250UL);
    TEST_ASSERT(config.yield_time.write_stdin_input.max_ms == 45000UL);
    TEST_ASSERT(config.yield_time.write_stdin_input.min_ms == 250UL);
    TEST_ASSERT(!config.sandbox_configured);

    const fs::path sandbox_config_path = root / "sandbox.ini";
    write_text(sandbox_config_path,
               "target = sandbox-cpp\n"
               "listen_host = 127.0.0.1\n"
               "listen_port = 8181\n"
               "default_workdir = /work\n"
               "sandbox_exec_cwd_allow = /work;/tmp/work\n"
               "sandbox_exec_cwd_deny = /work/private\n"
               "sandbox_read_allow = /work;/assets\n"
               "sandbox_read_deny = /work/.git;/assets/secrets\n"
               "sandbox_write_allow = /work\n"
               "sandbox_write_deny = /work/.git;/work/readonly\n");
    const DaemonConfig sandbox_config = load_config(sandbox_config_path.string());
    TEST_ASSERT(sandbox_config.port_forward_limits.max_worker_threads == DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
    TEST_ASSERT(sandbox_config.port_forward_limits.max_retained_sessions == DEFAULT_PORT_FORWARD_MAX_RETAINED_SESSIONS);
    TEST_ASSERT(sandbox_config.port_forward_limits.max_retained_listeners == DEFAULT_PORT_FORWARD_MAX_RETAINED_LISTENERS);
    TEST_ASSERT(sandbox_config.port_forward_limits.max_udp_binds == DEFAULT_PORT_FORWARD_MAX_UDP_BINDS);
    TEST_ASSERT(sandbox_config.port_forward_limits.max_active_tcp_streams == DEFAULT_PORT_FORWARD_MAX_ACTIVE_TCP_STREAMS);
    TEST_ASSERT(sandbox_config.port_forward_limits.max_tunnel_queued_bytes == DEFAULT_PORT_FORWARD_MAX_TUNNEL_QUEUED_BYTES);
    TEST_ASSERT(sandbox_config.port_forward_limits.tunnel_io_timeout_ms == DEFAULT_PORT_FORWARD_TUNNEL_IO_TIMEOUT_MS);
    TEST_ASSERT(sandbox_config.port_forward_limits.connect_timeout_ms == DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS);
    TEST_ASSERT(sandbox_config.http_connection_idle_timeout_ms == DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS);
    TEST_ASSERT(sandbox_config.transfer_limits.max_archive_bytes == DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES);
    TEST_ASSERT(sandbox_config.transfer_limits.max_entry_bytes == DEFAULT_TRANSFER_MAX_ENTRY_BYTES);
    TEST_ASSERT(sandbox_config.sandbox_configured);
    TEST_ASSERT(sandbox_config.sandbox.exec_cwd.allow.size() == 2);
    TEST_ASSERT(sandbox_config.sandbox.exec_cwd.allow[0] == "/work");
    TEST_ASSERT(sandbox_config.sandbox.exec_cwd.allow[1] == "/tmp/work");
    TEST_ASSERT(sandbox_config.sandbox.exec_cwd.deny[0] == "/work/private");
    TEST_ASSERT(sandbox_config.sandbox.read.allow[1] == "/assets");
    TEST_ASSERT(sandbox_config.sandbox.read.deny[1] == "/assets/secrets");
    TEST_ASSERT(sandbox_config.sandbox.write.allow[0] == "/work");
    TEST_ASSERT(sandbox_config.sandbox.write.deny[1] == "/work/readonly");

    const YieldTimeConfig defaults = YieldTimeConfig();
    TEST_ASSERT(resolve_yield_time_ms(defaults.exec_command, false, 0UL) == DEFAULT_YIELD_TIME_EXEC_COMMAND_DEFAULT_MS);
    TEST_ASSERT(resolve_yield_time_ms(defaults.exec_command, true, 1UL) == 250UL);
    TEST_ASSERT(resolve_yield_time_ms(defaults.write_stdin_poll, false, 0UL) == DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_DEFAULT_MS);
    TEST_ASSERT(resolve_yield_time_ms(defaults.write_stdin_poll, true, 1UL) == DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MIN_MS);
    TEST_ASSERT(resolve_yield_time_ms(defaults.write_stdin_poll, true, 400000UL) == DEFAULT_YIELD_TIME_WRITE_STDIN_POLL_MAX_MS);
    TEST_ASSERT(resolve_yield_time_ms(defaults.write_stdin_input, false, 0UL) == DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_DEFAULT_MS);
    TEST_ASSERT(resolve_yield_time_ms(defaults.write_stdin_input, true, 50000UL) == DEFAULT_YIELD_TIME_WRITE_STDIN_INPUT_MAX_MS);

    const fs::path invalid_path = root / "invalid.ini";
    write_text(invalid_path, "target builder-cpp\n");
    TEST_ASSERT(config_rejected(invalid_path));

    const fs::path invalid_worker_limit_path = root / "invalid-worker-limit.ini";
    write_text(invalid_worker_limit_path,
               "target = builder-cpp\n"
               "listen_host = 0.0.0.0\n"
               "listen_port = 8181\n"
               "default_workdir = C:\\work\n"
               "port_forward_max_worker_threads = 0\n");
    TEST_ASSERT(config_rejected(invalid_worker_limit_path));

    const fs::path invalid_transfer_zero_path = root / "invalid-transfer-zero.ini";
    write_text(invalid_transfer_zero_path,
               minimal_config_text() + "transfer_max_archive_bytes = 0\n"
                                       "transfer_max_entry_bytes = 1\n");
    TEST_ASSERT(config_rejected(invalid_transfer_zero_path));

    const fs::path invalid_transfer_bounds_path = root / "invalid-transfer-bounds.ini";
    write_text(invalid_transfer_bounds_path,
               minimal_config_text() + "transfer_max_archive_bytes = 8\n"
                                       "transfer_max_entry_bytes = 9\n");
    TEST_ASSERT(config_rejected(invalid_transfer_bounds_path));

    const char* invalid_limit_keys[] = {
        "http_connection_idle_timeout_ms",
        "port_forward_max_retained_sessions",
        "port_forward_max_retained_listeners",
        "port_forward_max_udp_binds",
        "port_forward_max_active_tcp_streams",
        "port_forward_max_tunnel_queued_bytes",
        "port_forward_tunnel_io_timeout_ms",
        "port_forward_connect_timeout_ms",
    };
    for (std::size_t index = 0; index < sizeof(invalid_limit_keys) / sizeof(invalid_limit_keys[0]); ++index) {
        const fs::path invalid_limit_path = root / ("invalid-" + std::string(invalid_limit_keys[index]) + ".ini");
        write_text(invalid_limit_path, minimal_config_text() + invalid_limit_keys[index] + " = 0\n");
        TEST_ASSERT(config_rejected(invalid_limit_path));
    }

    const fs::path invalid_yield_path = root / "invalid-yield.ini";
    write_text(invalid_yield_path,
               "target = builder-cpp\n"
               "listen_host = 0.0.0.0\n"
               "listen_port = 8181\n"
               "default_workdir = C:\\work\n"
               "yield_time_exec_command_default_ms = 10\n"
               "yield_time_exec_command_min_ms = 20\n");
    TEST_ASSERT(config_rejected(invalid_yield_path));

    const fs::path invalid_auth_path = root / "invalid-auth.ini";
    write_text(invalid_auth_path,
               "target = builder-cpp\n"
               "listen_host = 0.0.0.0\n"
               "listen_port = 8181\n"
               "default_workdir = C:\\work\n"
               "http_auth_bearer_token = bad token\n");
    TEST_ASSERT(config_rejected(invalid_auth_path));

    const fs::path invalid_port_path = root / "invalid-port.ini";
    write_text(invalid_port_path,
               "target = builder-cpp\n"
               "listen_host = 0.0.0.0\n"
               "listen_port = 70000\n"
               "default_workdir = C:\\work\n");
    TEST_ASSERT(config_rejected(invalid_port_path));

    return 0;
}
