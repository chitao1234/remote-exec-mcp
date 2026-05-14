#include <cassert>
#include <string>

#include "filesystem_sandbox.h"
#include "path_compare.h"
#include "path_policy.h"
#include "test_filesystem.h"

namespace fs = test_fs;

static fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-sandbox-test";
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

static bool denied(const CompiledFilesystemSandbox* sandbox, SandboxAccess access, const std::string& path) {
    try {
        authorize_path(sandbox, access, path);
        return false;
    } catch (const SandboxError&) {
        return true;
    }
}

int main() {
    const fs::path root = make_test_root();
    const fs::path allowed = root / "allowed";
    const fs::path denied_root = allowed / "secret";
    const fs::path sibling = root / "allowed-sibling";
    fs::create_directories(allowed);
    fs::create_directories(denied_root);
    fs::create_directories(sibling);

    FilesystemSandbox write_sandbox;
    write_sandbox.write.allow.push_back(allowed.string());
    write_sandbox.write.deny.push_back(denied_root.string());
    const CompiledFilesystemSandbox compiled = compile_filesystem_sandbox(write_sandbox);

    authorize_path(&compiled, SANDBOX_WRITE, (allowed / "new.txt").string());
    assert(denied(&compiled, SANDBOX_WRITE, (denied_root / "key.txt").string()));
    assert(denied(&compiled, SANDBOX_WRITE, (sibling / "nope.txt").string()));

    FilesystemSandbox deny_only;
    deny_only.read.deny.push_back(denied_root.string());
    const CompiledFilesystemSandbox deny_only_compiled = compile_filesystem_sandbox(deny_only);
    authorize_path(&deny_only_compiled, SANDBOX_READ, (sibling / "ok.txt").string());
    assert(denied(&deny_only_compiled, SANDBOX_READ, (denied_root / "blocked.txt").string()));

    FilesystemSandbox invalid;
    invalid.exec_cwd.allow.push_back("relative/path");
    bool rejected = false;
    try {
        (void)compile_filesystem_sandbox(invalid);
    } catch (const SandboxError&) {
        rejected = true;
    }
    assert(rejected);

    const PathPolicy windows_policy = windows_path_policy();
    assert(is_absolute_for_policy(windows_policy, "C:/Work/file.txt"));
    assert(is_absolute_for_policy(windows_policy, "/c/Work/file.txt"));
    assert(is_absolute_for_policy(windows_policy, "/cygdrive/c/Work/file.txt"));
    assert(normalize_for_system(windows_policy, "/c/Work/file.txt") == "C:\\Work\\file.txt");
    assert(normalize_for_system(windows_policy, "/cygdrive/c/Work/file.txt") == "C:\\Work\\file.txt");
    assert(join_for_policy(windows_policy, "C:/Work", "nested/file.txt") == "C:\\Work\\nested\\file.txt");

#ifdef _WIN32
    assert(host_path_equal("/c/WORK/File.txt", "c:\\work\\file.txt"));
    assert(host_path_equal("C:\\RÉSUMÉ\\Ärger.txt", "c:/résumé/ärger.TXT"));
    assert(host_path_is_within("C:\\WORK\\SECRET\\key.txt", "c:/work/secret"));
    assert(!host_path_is_within("C:\\Worker\\out.txt", "c:/work"));

    FilesystemSandbox windows_sandbox;
    windows_sandbox.read.allow.push_back("C:\\Work");
    windows_sandbox.read.deny.push_back("C:\\Work\\Secret");
    const CompiledFilesystemSandbox windows_compiled = compile_filesystem_sandbox(windows_sandbox);
    authorize_path(&windows_compiled, SANDBOX_READ, "c:/work/out.txt");
    assert(denied(&windows_compiled, SANDBOX_READ, "C:\\WORK\\SECRET\\key.txt"));
    assert(denied(&windows_compiled, SANDBOX_READ, "C:\\Worker\\out.txt"));

    FilesystemSandbox unicode_windows_sandbox;
    unicode_windows_sandbox.read.allow.push_back("C:\\RÉSUMÉ");
    unicode_windows_sandbox.read.deny.push_back("C:\\RÉSUMÉ\\Ärger");
    const CompiledFilesystemSandbox unicode_windows_compiled = compile_filesystem_sandbox(unicode_windows_sandbox);
    authorize_path(&unicode_windows_compiled, SANDBOX_READ, "c:/résumé/out.txt");
    assert(denied(&unicode_windows_compiled, SANDBOX_READ, "C:\\résumé\\ärger\\key.txt"));
#endif

    return 0;
}
