#include <cassert>
#include <filesystem>
#include <fstream>
#include <limits>
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

static void write_text_file(const fs::path& path, const std::string& contents) {
    std::ofstream out(path.string().c_str(), std::ios::binary);
    assert(out.good());
    out << contents;
    out.close();
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
        std::numeric_limits<unsigned long>::max(),
        yield_time
    );

    assert(response.at("daemon_session_id").is_null());
    assert(!response.at("running").get<bool>());
    assert(response.at("exit_code").get<int>() == 0);
    assert(
        normalize_output(response.at("output").get<std::string>()) ==
        "stdout-1\nstderr-1\nstdout-2\nstderr-2\n"
    );

    write_text_file(root / "long.txt", std::string(100, 'a'));
    const Json middle_truncated = store.start_command(
        "type long.txt",
        root.string(),
        "",
        true,
        5000UL,
        15UL,
        yield_time
    );
    assert(middle_truncated.at("original_token_count").get<int>() == 25);
    assert(
        normalize_output(middle_truncated.at("output").get<std::string>()) ==
        std::string("Total output lines: 1\n\naaaaaa") +
            "\xE2\x80\xA6" + "22 tokens truncated" + "\xE2\x80\xA6" + "aaaaaa"
    );

    write_text_file(root / "huge.txt", std::string(50000, 'x'));
    const Json omitted_limit = store.start_command(
        "type huge.txt",
        root.string(),
        "",
        true,
        5000UL,
        std::numeric_limits<unsigned long>::max(),
        yield_time
    );
    assert(omitted_limit.at("original_token_count").get<int>() == 12500);
    assert(
        normalize_output(omitted_limit.at("output").get<std::string>())
            .find("Total output lines: 1\n\n") == 0
    );
    assert(
        omitted_limit.at("output").get<std::string>().find("tokens truncated") !=
        std::string::npos
    );

    const Json zero_limit = store.start_command(
        "type huge.txt",
        root.string(),
        "",
        true,
        5000UL,
        0UL,
        yield_time
    );
    assert(zero_limit.at("original_token_count").get<int>() == 12500);
    assert(zero_limit.at("output").get<std::string>().empty());

    return 0;
}
