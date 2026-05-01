#include <cassert>
#include <filesystem>
#include <string>

#include "filesystem_sandbox.h"
#include "path_policy.h"

namespace fs = std::filesystem;

static fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-sandbox-test";
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

static bool denied(
    PathPolicy policy,
    const CompiledFilesystemSandbox* sandbox,
    SandboxAccess access,
    const std::string& path
) {
    try {
        authorize_path(policy, sandbox, access, path);
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

    const PathPolicy host_policy = host_path_policy();

    FilesystemSandbox write_sandbox;
    write_sandbox.write.allow.push_back(allowed.string());
    write_sandbox.write.deny.push_back(denied_root.string());
    const CompiledFilesystemSandbox compiled =
        compile_filesystem_sandbox(host_policy, write_sandbox);

    authorize_path(host_policy, &compiled, SANDBOX_WRITE, (allowed / "new.txt").string());
    assert(denied(host_policy, &compiled, SANDBOX_WRITE, (denied_root / "key.txt").string()));
    assert(denied(host_policy, &compiled, SANDBOX_WRITE, (sibling / "nope.txt").string()));

    FilesystemSandbox deny_only;
    deny_only.read.deny.push_back(denied_root.string());
    const CompiledFilesystemSandbox deny_only_compiled =
        compile_filesystem_sandbox(host_policy, deny_only);
    authorize_path(host_policy, &deny_only_compiled, SANDBOX_READ, (sibling / "ok.txt").string());
    assert(denied(
        host_policy,
        &deny_only_compiled,
        SANDBOX_READ,
        (denied_root / "blocked.txt").string()
    ));

    FilesystemSandbox invalid;
    invalid.exec_cwd.allow.push_back("relative/path");
    bool rejected = false;
    try {
        (void)compile_filesystem_sandbox(host_policy, invalid);
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
    assert(same_path_for_policy(windows_policy, "/c/WORK/File.txt", "c:\\work\\file.txt"));

    FilesystemSandbox windows_sandbox;
    windows_sandbox.read.allow.push_back("C:\\Work");
    windows_sandbox.read.deny.push_back("C:\\Work\\Secret");
    const CompiledFilesystemSandbox windows_compiled =
        compile_filesystem_sandbox(windows_policy, windows_sandbox);
    authorize_path(windows_policy, &windows_compiled, SANDBOX_READ, "c:/work/out.txt");
    assert(denied(windows_policy, &windows_compiled, SANDBOX_READ, "C:\\WORK\\SECRET\\key.txt"));
    assert(denied(windows_policy, &windows_compiled, SANDBOX_READ, "C:\\Worker\\out.txt"));

    return 0;
}
