#include <cassert>
#include <filesystem>
#include <fstream>
#include <stdexcept>
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

static void expect_patch_failure(const fs::path& root, const std::string& patch) {
    bool threw = false;
    try {
        (void)apply_patch(root.string(), patch);
    } catch (const std::runtime_error&) {
        threw = true;
    }
    assert(threw);
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
    assert(update_result.updated_paths.size() == 1);
    assert(update_result.updated_paths[0] == "M hello.txt");
    assert(read_text(root / "hello.txt") == "hello xp\n");

    write_text(root / "crlf.txt", "hello\r\nworld\r\n");
    const std::string crlf_patch =
        "*** Begin Patch\n"
        "*** Update File: crlf.txt\n"
        "@@\n"
        "-hello\n"
        "+hello xp\n"
        "*** End Patch\n";

    PatchApplyResult crlf_result = apply_patch(root.string(), crlf_patch);
    assert(crlf_result.output.find("M crlf.txt") != std::string::npos);
    assert(read_text(root / "crlf.txt") == "hello xp\r\nworld\r\n");

    const std::string add_patch =
        "*** Begin Patch\n"
        "*** Add File: new.txt\n"
        "+new file\n"
        "*** End Patch\n";

    PatchApplyResult add_result = apply_patch(root.string(), add_patch);
    assert(add_result.output.find("A new.txt") != std::string::npos);
    assert(read_text(root / "new.txt") == "new file\n");

    const fs::path absolute_path = root / "absolute.txt";
    const std::string absolute_add_patch =
        "*** Begin Patch\n"
        "*** Add File: " + absolute_path.string() + "\n"
        "+absolute file\n"
        "*** End Patch\n";

    PatchApplyResult absolute_add_result = apply_patch(root.string(), absolute_add_patch);
    assert(absolute_add_result.output.find("A ") != std::string::npos);
    assert(read_text(absolute_path) == "absolute file\n");

    const std::string absolute_update_patch =
        "*** Begin Patch\n"
        "*** Update File: " + absolute_path.string() + "\n"
        "@@\n"
        "-absolute file\n"
        "+absolute update\n"
        "*** End Patch\n";

    PatchApplyResult absolute_update_result = apply_patch(root.string(), absolute_update_patch);
    assert(absolute_update_result.output.find("M ") != std::string::npos);
    assert(read_text(absolute_path) == "absolute update\n");

    const std::string move_patch =
        "*** Begin Patch\n"
        "*** Update File: new.txt\n"
        "*** Move to: moved.txt\n"
        "@@\n"
        "-new file\n"
        "+moved file\n"
        "*** End Patch\n";

    PatchApplyResult move_result = apply_patch(root.string(), move_patch);
    assert(move_result.output.find("M moved.txt") != std::string::npos);
    assert(!fs::exists(root / "new.txt"));
    assert(read_text(root / "moved.txt") == "moved file\n");

    const std::string delete_patch =
        "*** Begin Patch\n"
        "*** Delete File: moved.txt\n"
        "*** End Patch\n";

    PatchApplyResult delete_result = apply_patch(root.string(), delete_patch);
    assert(delete_result.output.find("D moved.txt") != std::string::npos);
    assert(!fs::exists(root / "moved.txt"));

    write_text(root / "missing-header.txt", "before\nmiddle\n");
    const std::string missing_header_patch =
        "*** Begin Patch\n"
        "*** Update File: missing-header.txt\n"
        "-before\n"
        "+after\n"
        "*** End Patch\n";

    PatchApplyResult missing_header_result = apply_patch(root.string(), missing_header_patch);
    assert(missing_header_result.output.find("M missing-header.txt") != std::string::npos);
    assert(read_text(root / "missing-header.txt") == "after\nmiddle\n");

    write_text(root / "plain-add.txt", "alpha\nbeta\n");
    const std::string plain_add_patch =
        "*** Begin Patch\n"
        "*** Update File: plain-add.txt\n"
        "+gamma\n"
        "*** End Patch\n";

    PatchApplyResult plain_add_result = apply_patch(root.string(), plain_add_patch);
    assert(plain_add_result.output.find("M plain-add.txt") != std::string::npos);
    assert(read_text(root / "plain-add.txt") == "alpha\nbeta\ngamma\n");

    write_text(root / "eof.txt", "before\nmiddle\nbefore\n");
    const std::string eof_patch =
        "*** Begin Patch\n"
        "*** Update File: eof.txt\n"
        "@@\n"
        "-before\n"
        "+after\n"
        "*** End of File\n"
        "*** End Patch\n";

    PatchApplyResult eof_result = apply_patch(root.string(), eof_patch);
    assert(eof_result.output.find("M eof.txt") != std::string::npos);
    assert(read_text(root / "eof.txt") == "before\nmiddle\nafter\n");

    write_text(root / "eof-fail.txt", "before\nmiddle\ntail\n");
    const std::string eof_fail_patch =
        "*** Begin Patch\n"
        "*** Update File: eof-fail.txt\n"
        "@@\n"
        "-before\n"
        "+after\n"
        "*** End of File\n"
        "*** End Patch\n";

    expect_patch_failure(root, eof_fail_patch);
    assert(read_text(root / "eof-fail.txt") == "before\nmiddle\ntail\n");

    write_text(root / "append.txt", "before\ntail\n");
    const std::string append_patch =
        "*** Begin Patch\n"
        "*** Update File: append.txt\n"
        "@@ tail\n"
        "+after\n"
        "*** End of File\n"
        "*** End Patch\n";

    PatchApplyResult append_result = apply_patch(root.string(), append_patch);
    assert(append_result.output.find("M append.txt") != std::string::npos);
    assert(read_text(root / "append.txt") == "before\ntail\nafter\n");

    write_text(root / "append-repeat.txt", "before\ntail\nmiddle\ntail\n");
    const std::string append_repeat_patch =
        "*** Begin Patch\n"
        "*** Update File: append-repeat.txt\n"
        "@@ tail\n"
        "+after\n"
        "*** End of File\n"
        "*** End Patch\n";

    PatchApplyResult append_repeat_result = apply_patch(root.string(), append_repeat_patch);
    assert(append_repeat_result.output.find("M append-repeat.txt") != std::string::npos);
    assert(read_text(root / "append-repeat.txt") == "before\ntail\nmiddle\ntail\nafter\n");

    write_text(root / "repeat.txt", "a\nmarker\nb\nmarker\nc\n");
    const std::string repeat_patch =
        "*** Begin Patch\n"
        "*** Update File: repeat.txt\n"
        "@@ marker\n"
        "+first\n"
        "@@ marker\n"
        "+second\n"
        "*** End Patch\n";

    PatchApplyResult repeat_result = apply_patch(root.string(), repeat_patch);
    assert(repeat_result.output.find("M repeat.txt") != std::string::npos);
    assert(read_text(root / "repeat.txt") == "a\nfirst\nmarker\nb\nsecond\nmarker\nc\n");

    write_text(root / "repeat-replace.txt", "old\nmarker\nold\nmarker\nold\n");
    const std::string repeat_replace_patch =
        "*** Begin Patch\n"
        "*** Update File: repeat-replace.txt\n"
        "@@ marker\n"
        "-old\n"
        "+first\n"
        "@@ marker\n"
        "-old\n"
        "+second\n"
        "*** End Patch\n";

    PatchApplyResult repeat_replace_result = apply_patch(root.string(), repeat_replace_patch);
    assert(repeat_replace_result.output.find("M repeat-replace.txt") != std::string::npos);
    assert(read_text(root / "repeat-replace.txt") == "old\nmarker\nfirst\nmarker\nsecond\n");

    write_text(root / "partial-first.txt", "before\n");
    const std::string partial_patch =
        "*** Begin Patch\n"
        "*** Update File: partial-first.txt\n"
        "@@\n"
        "-before\n"
        "+after\n"
        "*** Delete File: missing.txt\n"
        "*** End Patch\n";

    expect_patch_failure(root, partial_patch);
    assert(read_text(root / "partial-first.txt") == "after\n");

    const fs::path blocked_path = root / "blocked.txt";
    fs::create_directories(blocked_path);
    write_text(blocked_path / "child.txt", "child\n");
    const std::string blocked_add_patch =
        "*** Begin Patch\n"
        "*** Add File: blocked.txt\n"
        "+blocked\n"
        "*** End Patch\n";

    expect_patch_failure(root, blocked_add_patch);
    assert(!fs::exists(root / "blocked.txt.tmp"));

    return 0;
}
