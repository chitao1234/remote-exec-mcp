#include <cassert>
#include <filesystem>
#include <string>

#include "config.h"
#include "session_store.h"

namespace fs = std::filesystem;

static fs::path make_test_root() {
    char buffer[MAX_PATH];
    const DWORD length = GetTempPathA(MAX_PATH, buffer);
    assert(length > 0);
    assert(length < MAX_PATH);

    const fs::path root = fs::path(buffer) / "remote-exec-xp-session-store-test";
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

    const Json response = store.start_command(
        "echo stdout-1 & echo stderr-1 1>&2 & echo stdout-2 & echo stderr-2 1>&2",
        root.string(),
        "",
        true,
        5000UL,
        0UL,
        yield_time
    );

    assert(response.at("daemon_session_id").is_null());
    assert(!response.at("running").get<bool>());
    assert(response.at("exit_code").get<int>() == 0);
    assert(
        normalize_output(response.at("output").get<std::string>()) ==
        "stdout-1\nstderr-1\nstdout-2\nstderr-2\n"
    );

    return 0;
}
