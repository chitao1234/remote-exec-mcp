#include "test_assert.h"
#include <string>

#include "filesystem_sandbox.h"
#include "path_compare.h"
#include "path_policy.h"
#include "test_contract_fixtures.h"
#include "test_filesystem.h"

namespace fs = test_fs;

namespace {

fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-sandbox-test";
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

bool denied(const CompiledFilesystemSandbox* sandbox, SandboxAccess access, const std::string& path) {
    try {
        authorize_path(sandbox, access, path);
        return false;
    } catch (const SandboxError&) {
        return true;
    }
}

std::string host_platform_label() {
#ifdef _WIN32
    return "windows";
#else
    return "posix";
#endif
}

PathPolicy policy_for_style(const std::string& style) {
    if (style == "posix") {
        return posix_path_policy();
    }
    if (style == "windows") {
        return windows_path_policy();
    }
    throw std::runtime_error("unknown path policy style `" + style + "`");
}

SandboxAccess access_for_label(const std::string& label) {
    if (label == "exec_cwd") {
        return SANDBOX_EXEC_CWD;
    }
    if (label == "read") {
        return SANDBOX_READ;
    }
    if (label == "write") {
        return SANDBOX_WRITE;
    }
    throw std::runtime_error("unknown sandbox access `" + label + "`");
}

std::string replace_all(std::string value, const std::string& needle, const std::string& replacement) {
    std::string::size_type position = 0;
    while ((position = value.find(needle, position)) != std::string::npos) {
        value.replace(position, needle.size(), replacement);
        position += replacement.size();
    }
    return value;
}

std::string apply_template(const std::string& raw, const fs::path& root) {
    return replace_all(raw, "{root}", root.string());
}

bool case_applies_to_host(const Json& case_json) {
    if (!case_json.contains("platforms")) {
        return true;
    }
    const std::string platform = host_platform_label();
    const Json& platforms = case_json.at("platforms");
    for (Json::const_iterator it = platforms.begin(); it != platforms.end(); ++it) {
        if (it->get<std::string>() == platform) {
            return true;
        }
    }
    return false;
}

void apply_setup(const fs::path& root, const Json& setup) {
    if (setup.is_null()) {
        return;
    }

    if (setup.contains("dirs")) {
        const Json& dirs = setup.at("dirs");
        for (Json::const_iterator it = dirs.begin(); it != dirs.end(); ++it) {
            fs::create_directories(apply_template(it->get<std::string>(), root));
        }
    }

    if (setup.contains("files")) {
        const Json& files = setup.at("files");
        for (Json::const_iterator it = files.begin(); it != files.end(); ++it) {
            const fs::path path = apply_template(it->at("path").get<std::string>(), root);
            fs::create_directories(path.parent_path());
            fs::write_file_bytes(path, it->at("contents").get<std::string>());
        }
    }

#ifndef _WIN32
    if (setup.contains("symlinks")) {
        const Json& symlinks = setup.at("symlinks");
        for (Json::const_iterator it = symlinks.begin(); it != symlinks.end(); ++it) {
            const fs::path link = apply_template(it->at("path").get<std::string>(), root);
            fs::create_directories(link.parent_path());
            fs::create_symlink(apply_template(it->at("target").get<std::string>(), root), link);
        }
    }
#endif
}

void assign_sandbox_list(FilesystemSandbox* sandbox, SandboxAccess access, const SandboxPathList& list) {
    switch (access) {
    case SANDBOX_EXEC_CWD:
        sandbox->exec_cwd = list;
        return;
    case SANDBOX_READ:
        sandbox->read = list;
        return;
    case SANDBOX_WRITE:
        sandbox->write = list;
        return;
    }
}

FilesystemSandbox sandbox_for_case(const Json& case_json, const fs::path& root) {
    SandboxPathList list;
    const Json& allow = case_json.at("allow");
    for (Json::const_iterator it = allow.begin(); it != allow.end(); ++it) {
        list.allow.push_back(apply_template(it->get<std::string>(), root));
    }
    const Json& deny = case_json.at("deny");
    for (Json::const_iterator it = deny.begin(); it != deny.end(); ++it) {
        list.deny.push_back(apply_template(it->get<std::string>(), root));
    }

    FilesystemSandbox sandbox;
    assign_sandbox_list(&sandbox, access_for_label(case_json.at("access").get<std::string>()), list);
    return sandbox;
}

void run_shared_path_policy_cases() {
    const Json& cases = test_contract::path_policy_cases();

    const Json& absolute_cases = cases.at("is_absolute");
    for (Json::const_iterator it = absolute_cases.begin(); it != absolute_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(is_absolute_for_policy(policy, it->at("raw").get<std::string>()) == it->at("expected").get<bool>());
    }

    const Json& normalize_cases = cases.at("normalize_for_system");
    for (Json::const_iterator it = normalize_cases.begin(); it != normalize_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(normalize_for_system(policy, it->at("raw").get<std::string>()) ==
                    it->at("expected").get<std::string>());
    }

    const Json& syntax_cases = cases.at("syntax_eq");
    for (Json::const_iterator it = syntax_cases.begin(); it != syntax_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(syntax_eq_for_policy(
                        policy, it->at("left").get<std::string>(), it->at("right").get<std::string>()) ==
                    it->at("expected").get<bool>());
    }

    const Json& join_cases = cases.at("join");
    for (Json::const_iterator it = join_cases.begin(); it != join_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(join_for_policy(
                        policy, it->at("base").get<std::string>(), it->at("child").get<std::string>()) ==
                    it->at("expected").get<std::string>());
    }

    const Json& basename_cases = cases.at("basename");
    for (Json::const_iterator it = basename_cases.begin(); it != basename_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        std::string basename;
        const bool found = basename_for_policy(policy, it->at("raw").get<std::string>(), &basename);
        if (it->at("expected").is_null()) {
            TEST_ASSERT(!found);
        } else {
            TEST_ASSERT(found);
            TEST_ASSERT(basename == it->at("expected").get<std::string>());
        }
    }
}

void run_shared_path_compare_cases() {
    const Json& cases = test_contract::path_compare_cases();
    const std::string platform = host_platform_label();

    const Json& equal_cases = cases.at("path_eq");
    for (Json::const_iterator it = equal_cases.begin(); it != equal_cases.end(); ++it) {
        if (it->at("platform").get<std::string>() != platform) {
            continue;
        }
        TEST_ASSERT(host_path_equal(it->at("left").get<std::string>(), it->at("right").get<std::string>()) ==
                    it->at("expected").get<bool>());
    }

    const Json& prefix_cases = cases.at("path_has_prefix");
    for (Json::const_iterator it = prefix_cases.begin(); it != prefix_cases.end(); ++it) {
        if (it->at("platform").get<std::string>() != platform) {
            continue;
        }
        TEST_ASSERT(host_path_has_prefix(it->at("path").get<std::string>(), it->at("prefix").get<std::string>()) ==
                    it->at("expected").get<bool>());
    }

    const Json& within_cases = cases.at("path_is_within");
    for (Json::const_iterator it = within_cases.begin(); it != within_cases.end(); ++it) {
        if (it->at("platform").get<std::string>() != platform) {
            continue;
        }
        TEST_ASSERT(host_path_is_within(it->at("path").get<std::string>(), it->at("root").get<std::string>()) ==
                    it->at("expected").get<bool>());
    }
}

void run_shared_sandbox_cases() {
    const Json& cases = test_contract::sandbox_cases();

    const Json& compile_cases = cases.at("compile");
    for (Json::const_iterator it = compile_cases.begin(); it != compile_cases.end(); ++it) {
        const fs::path root = make_test_root();
        apply_setup(root, it->contains("setup") ? it->at("setup") : Json());
        const FilesystemSandbox sandbox = sandbox_for_case(*it, root);
        const std::string expected = it->at("expected").get<std::string>();

        if (expected == "ok") {
            (void)compile_filesystem_sandbox(sandbox);
            continue;
        }

        bool failed = false;
        try {
            (void)compile_filesystem_sandbox(sandbox);
        } catch (const SandboxError& ex) {
            failed = true;
            if (it->contains("expected_message_fragment")) {
                TEST_ASSERT(std::string(ex.what()).find(it->at("expected_message_fragment").get<std::string>()) !=
                            std::string::npos);
            }
        }
        TEST_ASSERT(failed);
    }

    const Json& authorize_cases = cases.at("authorize");
    for (Json::const_iterator it = authorize_cases.begin(); it != authorize_cases.end(); ++it) {
        if (!case_applies_to_host(*it)) {
            continue;
        }

        const fs::path root = make_test_root();
        apply_setup(root, it->contains("setup") ? it->at("setup") : Json());
        const FilesystemSandbox sandbox = sandbox_for_case(*it, root);
        const CompiledFilesystemSandbox compiled = compile_filesystem_sandbox(sandbox);
        const std::string path = apply_template(it->at("path").get<std::string>(), root);
        const SandboxAccess access = access_for_label(it->at("access").get<std::string>());
        const std::string expected = it->at("expected").get<std::string>();

        if (expected == "allow") {
            authorize_path(&compiled, access, path);
            continue;
        }

        bool failed = false;
        try {
            authorize_path(&compiled, access, path);
        } catch (const SandboxError& ex) {
            failed = true;
            if (it->contains("expected_message_fragment")) {
                TEST_ASSERT(std::string(ex.what()).find(it->at("expected_message_fragment").get<std::string>()) !=
                            std::string::npos);
            }
        }
        TEST_ASSERT(failed);
    }
}

} // namespace

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
    TEST_ASSERT(denied(&compiled, SANDBOX_WRITE, (denied_root / "key.txt").string()));
    TEST_ASSERT(denied(&compiled, SANDBOX_WRITE, (sibling / "nope.txt").string()));

    FilesystemSandbox deny_only;
    deny_only.read.deny.push_back(denied_root.string());
    const CompiledFilesystemSandbox deny_only_compiled = compile_filesystem_sandbox(deny_only);
    authorize_path(&deny_only_compiled, SANDBOX_READ, (sibling / "ok.txt").string());
    TEST_ASSERT(denied(&deny_only_compiled, SANDBOX_READ, (denied_root / "blocked.txt").string()));

    FilesystemSandbox invalid;
    invalid.exec_cwd.allow.push_back("relative/path");
    bool rejected = false;
    try {
        (void)compile_filesystem_sandbox(invalid);
    } catch (const SandboxError&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);

    const PathPolicy windows_policy = windows_path_policy();
    TEST_ASSERT(is_absolute_for_policy(windows_policy, "C:/Work/file.txt"));
    TEST_ASSERT(is_absolute_for_policy(windows_policy, "/c/Work/file.txt"));
    TEST_ASSERT(is_absolute_for_policy(windows_policy, "/cygdrive/c/Work/file.txt"));
    TEST_ASSERT(normalize_for_system(windows_policy, "/c/Work/file.txt") == "C:\\Work\\file.txt");
    TEST_ASSERT(normalize_for_system(windows_policy, "/cygdrive/c/Work/file.txt") == "C:\\Work\\file.txt");
    TEST_ASSERT(join_for_policy(windows_policy, "C:/Work", "nested/file.txt") == "C:\\Work\\nested\\file.txt");

#ifdef _WIN32
    TEST_ASSERT(host_path_equal("/c/WORK/File.txt", "c:\\work\\file.txt"));
    TEST_ASSERT(host_path_equal("C:\\RÉSUMÉ\\Ärger.txt", "c:/résumé/ärger.TXT"));
    TEST_ASSERT(host_path_is_within("C:\\WORK\\SECRET\\key.txt", "c:/work/secret"));
    TEST_ASSERT(!host_path_is_within("C:\\Worker\\out.txt", "c:/work"));

    FilesystemSandbox windows_sandbox;
    windows_sandbox.read.allow.push_back("C:\\Work");
    windows_sandbox.read.deny.push_back("C:\\Work\\Secret");
    const CompiledFilesystemSandbox windows_compiled = compile_filesystem_sandbox(windows_sandbox);
    authorize_path(&windows_compiled, SANDBOX_READ, "c:/work/out.txt");
    TEST_ASSERT(denied(&windows_compiled, SANDBOX_READ, "C:\\WORK\\SECRET\\key.txt"));
    TEST_ASSERT(denied(&windows_compiled, SANDBOX_READ, "C:\\Worker\\out.txt"));

    FilesystemSandbox unicode_windows_sandbox;
    unicode_windows_sandbox.read.allow.push_back("C:\\RÉSUMÉ");
    unicode_windows_sandbox.read.deny.push_back("C:\\RÉSUMÉ\\Ärger");
    const CompiledFilesystemSandbox unicode_windows_compiled = compile_filesystem_sandbox(unicode_windows_sandbox);
    authorize_path(&unicode_windows_compiled, SANDBOX_READ, "c:/résumé/out.txt");
    TEST_ASSERT(denied(&unicode_windows_compiled, SANDBOX_READ, "C:\\résumé\\ärger\\key.txt"));
#endif

    run_shared_path_policy_cases();
    run_shared_path_compare_cases();
    run_shared_sandbox_cases();

    return 0;
}
