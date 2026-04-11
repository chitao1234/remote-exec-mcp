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
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-config-test";
    fs::remove_all(root);
    fs::create_directories(root);

    const fs::path config_path = root / "daemon-xp.ini";
    write_text(
        config_path,
        "# comment\n"
        "target = builder-xp\n"
        "listen_host = 0.0.0.0\n"
        "listen_port = 8181\n"
        "default_workdir = \"C:\\work dir\"\n"
        "http_auth_bearer_token = shared-secret\n"
        "yield_time_exec_command_default_ms = 15000\n"
        "yield_time_exec_command_max_ms = 60000\n"
        "yield_time_exec_command_min_ms = 500\n"
        "yield_time_write_stdin_poll_default_ms = 12000\n"
        "yield_time_write_stdin_input_max_ms = 45000\n"
    );

    const DaemonConfig config = load_config(config_path.string());
    assert(config.target == "builder-xp");
    assert(config.listen_host == "0.0.0.0");
    assert(config.listen_port == 8181);
    assert(config.default_workdir == "C:\\work dir");
    assert(config.http_auth_bearer_token == "shared-secret");
    assert(config.yield_time.exec_command.default_ms == 15000UL);
    assert(config.yield_time.exec_command.max_ms == 60000UL);
    assert(config.yield_time.exec_command.min_ms == 500UL);
    assert(config.yield_time.write_stdin_poll.default_ms == 12000UL);
    assert(config.yield_time.write_stdin_poll.max_ms == 300000UL);
    assert(config.yield_time.write_stdin_poll.min_ms == 5000UL);
    assert(config.yield_time.write_stdin_input.default_ms == 250UL);
    assert(config.yield_time.write_stdin_input.max_ms == 45000UL);
    assert(config.yield_time.write_stdin_input.min_ms == 250UL);

    const YieldTimeConfig defaults = default_yield_time_config();
    assert(resolve_yield_time_ms(defaults.exec_command, false, 0UL) == 10000UL);
    assert(resolve_yield_time_ms(defaults.exec_command, true, 1UL) == 250UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_poll, false, 0UL) == 5000UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_poll, true, 1UL) == 5000UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_poll, true, 400000UL) == 300000UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_input, false, 0UL) == 250UL);
    assert(resolve_yield_time_ms(defaults.write_stdin_input, true, 50000UL) == 30000UL);

    const fs::path invalid_path = root / "invalid.ini";
    write_text(invalid_path, "target builder-xp\n");
    bool rejected = false;
    try {
        (void)load_config(invalid_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    const fs::path invalid_yield_path = root / "invalid-yield.ini";
    write_text(
        invalid_yield_path,
        "target = builder-xp\n"
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
        "target = builder-xp\n"
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

    return 0;
}
