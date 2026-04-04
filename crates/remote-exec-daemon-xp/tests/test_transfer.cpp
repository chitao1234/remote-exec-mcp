#include <cassert>
#include <filesystem>
#include <fstream>
#include <string>

#include "transfer_ops.h"

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
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-test";
    fs::remove_all(root);
    fs::create_directories(root / "subdir");

    write_text(root / "source.txt", "hello transfer");
    const ExportedFile exported = export_file((root / "source.txt").string());
    assert(exported.source_type == "file");
    assert(exported.bytes == "hello transfer");

    const ImportSummary imported =
        import_file(exported.bytes, (root / "copied.txt").string(), true, true);
    assert(imported.files_copied == 1);
    assert(imported.directories_copied == 0);
    assert(read_text(root / "copied.txt") == "hello transfer");

    bool rejected_directory = false;
    try {
        export_file((root / "subdir").string());
    } catch (...) {
        rejected_directory = true;
    }
    assert(rejected_directory);

    return 0;
}
