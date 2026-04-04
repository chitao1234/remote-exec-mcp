#include <cassert>
#include <filesystem>
#include <fstream>
#include <string>

#include "patch_engine.h"

namespace fs = std::filesystem;

static std::string read_text(const fs::path& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

static void write_text(const fs::path& path, const std::string& value) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    output << value;
}

int main() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-patch-test";
    fs::remove_all(root);
    fs::create_directories(root);

    write_text(root / "hello.txt", "hello\n");
    const std::string update_patch =
        "*** Begin Patch\n"
        "*** Update File: hello.txt\n"
        "@@\n"
        "-hello\n"
        "+hello xp\n"
        "*** End Patch\n";

    PatchApplyResult update_result = apply_patch(root.string(), update_patch);
    assert(update_result.output.find("M hello.txt") != std::string::npos);
    assert(read_text(root / "hello.txt") == "hello xp\n");

    const std::string add_patch =
        "*** Begin Patch\n"
        "*** Add File: new.txt\n"
        "+new file\n"
        "*** End Patch\n";

    PatchApplyResult add_result = apply_patch(root.string(), add_patch);
    assert(add_result.output.find("A new.txt") != std::string::npos);
    assert(read_text(root / "new.txt") == "new file\n");

    const std::string move_patch =
        "*** Begin Patch\n"
        "*** Update File: new.txt\n"
        "*** Move to: moved.txt\n"
        "*** End Patch\n";

    PatchApplyResult move_result = apply_patch(root.string(), move_patch);
    assert(move_result.output.find("M moved.txt") != std::string::npos);
    assert(!fs::exists(root / "new.txt"));
    assert(read_text(root / "moved.txt") == "new file\n");

    const std::string delete_patch =
        "*** Begin Patch\n"
        "*** Delete File: moved.txt\n"
        "*** End Patch\n";

    PatchApplyResult delete_result = apply_patch(root.string(), delete_patch);
    assert(delete_result.output.find("D moved.txt") != std::string::npos);
    assert(!fs::exists(root / "moved.txt"));

    return 0;
}
