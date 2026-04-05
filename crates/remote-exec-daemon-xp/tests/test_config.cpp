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
    );

    const DaemonConfig config = load_config(config_path.string());
    assert(config.target == "builder-xp");
    assert(config.listen_host == "0.0.0.0");
    assert(config.listen_port == 8181);
    assert(config.default_workdir == "C:\\work dir");

    const fs::path invalid_path = root / "invalid.ini";
    write_text(invalid_path, "target builder-xp\n");
    bool rejected = false;
    try {
        (void)load_config(invalid_path.string());
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    return 0;
}
