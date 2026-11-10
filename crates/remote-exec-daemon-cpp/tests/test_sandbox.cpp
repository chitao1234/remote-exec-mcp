#include "test_assert.h"
#include <string>
#include <thread>

#include "policy/filesystem_sandbox.h"
#include "policy/path_compare.h"
#include "policy/path_policy.h"
#include "runtime/start_gate.h"
#include "test_contract_fixtures.h"
#include "test_filesystem.h"

#ifdef _WIN32
#include <windows.h>

#include "platform/win32_utf8.h"
#endif

namespace fs = test_fs;

namespace {

#ifdef _WIN32
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
bool ansi_represents_wide(const wchar_t* text) {
    BOOL used_default = FALSE;
    const int length = WideCharToMultiByte(CP_ACP, 0, text, -1, nullptr, 0, nullptr, &used_default);
    return length > 0 && used_default == FALSE;
}
#endif

bool ansi_represents_paths(const std::wstring* paths, std::size_t count) {
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    for (std::size_t i = 0; i < count; ++i) {
        if (!ansi_represents_wide(paths[i].c_str())) {
            return false;
        }
    }
#else
    (void)paths;
    (void)count;
#endif
    return true;
}

void assert_unicode_path_case_folding() {
    const std::wstring upper = L"C:\\R\u00C9SUM\u00C9\\\u00C4rger.txt";
    const std::wstring lower = L"c:/r\u00E9sum\u00E9/\u00E4rger.TXT";
#ifdef REMOTE_EXEC_CPP_WINDOWS_ANSI_API
    // The ANSI Win9x path API is limited to the active code page. Do not
    // assume Western accented characters are representable there (for
    // example, a CP936 host maps them through a lossy fallback).
    if (!ansi_represents_wide(upper.c_str()) || !ansi_represents_wide(lower.c_str())) {
        return;
    }
#endif
    TEST_ASSERT(
        host_path_equal(win32_utf8::utf8_from_wide(upper), win32_utf8::utf8_from_wide(lower))
    );
}

void test_concurrent_first_path_comparison() {
    StartGate start;
    std::vector<std::thread> workers;
    for (unsigned int i = 0; i < 8U; ++i) {
        workers.push_back(std::thread([&start]() {
            TEST_ASSERT(start.wait());
            for (unsigned int iteration = 0; iteration < 100U; ++iteration) {
                TEST_ASSERT(host_path_equal("C:/Work/File.txt", "c:\\work\\FILE.TXT"));
                TEST_ASSERT(!host_path_equal("C:/Work/File.txt", "c:\\work\\other.txt"));
                TEST_ASSERT(host_path_has_prefix("C:/Work/File.txt", "c:\\WORK"));
            }
        }));
    }
    start.release();
    for (std::size_t i = 0; i < workers.size(); ++i) {
        workers[i].join();
    }
}
#endif

fs::path make_test_root() {
    const fs::path root = fs::unique_test_root("remote-exec-cpp-sandbox-test");
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

bool path_text_supported(const std::string& utf8) {
#if defined(_WIN32) && defined(REMOTE_EXEC_CPP_WINDOWS_ANSI_API)
    try {
        return ansi_represents_wide(win32_utf8::wide_from_utf8(utf8).c_str());
    } catch (const std::exception&) {
        return false;
    }
#else
    (void)utf8;
    return true;
#endif
}

bool denied(
    const CompiledFilesystemSandbox* sandbox,
    SandboxAccess access,
    const std::string& path
) {
    try {
        authorize_path(sandbox, access, path);
        return false;
    } catch (const SandboxError&) {
        return true;
    }
}

#ifdef _WIN32
std::string short_path_for_test(const fs::path& path) {
    const remote_exec_win32::NativeString native = test_fs::native_from_utf8(path.string());
    DWORD length = remote_exec_win32::get_short_path_name_native(native.c_str(), NULL, 0);
    if (length == 0U) {
        return "";
    }
    std::vector<remote_exec_win32::NativeChar> buffer(length + 1U);
    length = remote_exec_win32::get_short_path_name_native(
        native.c_str(),
        &buffer[0],
        static_cast<DWORD>(buffer.size())
    );
    if (length == 0U || length >= buffer.size()) {
        return "";
    }
    return remote_exec_win32::utf8_from_native(
        remote_exec_win32::NativeString(&buffer[0], length),
        "GetShortPathName"
    );
}

bool distinct_short_path_for_test(const fs::path& path, std::string* short_path) {
    *short_path = short_path_for_test(path);
    return !short_path->empty() && !host_path_equal(*short_path, path.string());
}
#endif

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

void assign_sandbox_list(
    FilesystemSandbox* sandbox,
    SandboxAccess access,
    const SandboxPathList& list
) {
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
        list.allow.push_back(test_contract::apply_template(it->get<std::string>(), root));
    }
    const Json& deny = case_json.at("deny");
    for (Json::const_iterator it = deny.begin(); it != deny.end(); ++it) {
        list.deny.push_back(test_contract::apply_template(it->get<std::string>(), root));
    }

    FilesystemSandbox sandbox;
    assign_sandbox_list(
        &sandbox,
        access_for_label(case_json.at("access").get<std::string>()),
        list
    );
    return sandbox;
}

void run_shared_path_policy_cases() {
    const Json& cases = test_contract::path_policy_cases();

    const Json& absolute_cases = cases.at("is_absolute");
    for (Json::const_iterator it = absolute_cases.begin(); it != absolute_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(
            is_absolute_for_policy(policy, it->at("raw").get<std::string>())
            == it->at("expected").get<bool>()
        );
    }

    const Json& normalize_cases = cases.at("normalize_for_system");
    for (Json::const_iterator it = normalize_cases.begin(); it != normalize_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(
            normalize_for_system(policy, it->at("raw").get<std::string>())
            == it->at("expected").get<std::string>()
        );
    }

    const Json& syntax_cases = cases.at("syntax_eq");
    for (Json::const_iterator it = syntax_cases.begin(); it != syntax_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(
            syntax_eq_for_policy(
                policy,
                it->at("left").get<std::string>(),
                it->at("right").get<std::string>()
            )
            == it->at("expected").get<bool>()
        );
    }

    const Json& join_cases = cases.at("join");
    for (Json::const_iterator it = join_cases.begin(); it != join_cases.end(); ++it) {
        const PathPolicy policy = policy_for_style(it->at("style").get<std::string>());
        TEST_ASSERT(
            join_for_policy(
                policy,
                it->at("base").get<std::string>(),
                it->at("child").get<std::string>()
            )
            == it->at("expected").get<std::string>()
        );
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
        const std::string left = it->at("left").get<std::string>();
        const std::string right = it->at("right").get<std::string>();
        if (!path_text_supported(left) || !path_text_supported(right)) {
            continue;
        }
        TEST_ASSERT(host_path_equal(left, right) == it->at("expected").get<bool>());
    }

    const Json& prefix_cases = cases.at("path_has_prefix");
    for (Json::const_iterator it = prefix_cases.begin(); it != prefix_cases.end(); ++it) {
        if (it->at("platform").get<std::string>() != platform) {
            continue;
        }
        const std::string path = it->at("path").get<std::string>();
        const std::string prefix = it->at("prefix").get<std::string>();
        if (!path_text_supported(path) || !path_text_supported(prefix)) {
            continue;
        }
        TEST_ASSERT(host_path_has_prefix(path, prefix) == it->at("expected").get<bool>());
    }

    const Json& within_cases = cases.at("path_is_within");
    for (Json::const_iterator it = within_cases.begin(); it != within_cases.end(); ++it) {
        if (it->at("platform").get<std::string>() != platform) {
            continue;
        }
        const std::string path = it->at("path").get<std::string>();
        const std::string root = it->at("root").get<std::string>();
        if (!path_text_supported(path) || !path_text_supported(root)) {
            continue;
        }
        TEST_ASSERT(host_path_is_within(path, root) == it->at("expected").get<bool>());
    }
}

void run_shared_sandbox_cases() {
    const Json& cases = test_contract::sandbox_cases();

    const Json& compile_cases = cases.at("compile");
    for (Json::const_iterator it = compile_cases.begin(); it != compile_cases.end(); ++it) {
        const fs::path root = make_test_root();
        test_contract::apply_setup(root, it->contains("setup") ? it->at("setup") : Json());
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
                TEST_ASSERT(
                    std::string(ex.what()).find(
                        it->at("expected_message_fragment").get<std::string>()
                    )
                    != std::string::npos
                );
            }
        }
        TEST_ASSERT(failed);
    }

    const Json& authorize_cases = cases.at("authorize");
    for (Json::const_iterator it = authorize_cases.begin(); it != authorize_cases.end(); ++it) {
        if (!test_contract::case_applies_to_host(*it)) {
            continue;
        }

        const fs::path root = make_test_root();
        test_contract::apply_setup(root, it->contains("setup") ? it->at("setup") : Json());
        const FilesystemSandbox sandbox = sandbox_for_case(*it, root);
        const CompiledFilesystemSandbox compiled = compile_filesystem_sandbox(sandbox);
        const std::string path =
            test_contract::apply_template(it->at("path").get<std::string>(), root);
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
                TEST_ASSERT(
                    std::string(ex.what()).find(
                        it->at("expected_message_fragment").get<std::string>()
                    )
                    != std::string::npos
                );
            }
        }
        TEST_ASSERT(failed);
    }
}

#ifdef _WIN32
void run_windows_short_alias_sandbox_cases() {
    const fs::path root = make_test_root();
    const fs::path allowed = root / "Eight Three Allowed Directory";
    const fs::path public_root = allowed / "Eight Three Public Directory";
    const fs::path denied_root = allowed / "Eight Three Denied Directory";
    fs::create_directories(public_root);
    fs::create_directories(denied_root);
    fs::write_file_bytes(public_root / "ok.txt", "ok");
    fs::write_file_bytes(denied_root / "key.txt", "secret");

    std::string short_allowed;
    if (!distinct_short_path_for_test(allowed, &short_allowed)) {
        return;
    }

    FilesystemSandbox long_rule_sandbox;
    long_rule_sandbox.read.allow.push_back(allowed.string());
    long_rule_sandbox.read.deny.push_back(denied_root.string());
    const CompiledFilesystemSandbox long_rule_compiled =
        compile_filesystem_sandbox(long_rule_sandbox);
    authorize_path(
        &long_rule_compiled,
        SANDBOX_READ,
        short_allowed + "\\Eight Three Public Directory\\ok.txt"
    );
    authorize_path(
        &long_rule_compiled,
        SANDBOX_READ,
        short_allowed + "\\Eight Three Public Directory\\new.txt"
    );
    TEST_ASSERT(denied(
        &long_rule_compiled,
        SANDBOX_READ,
        short_allowed + "\\Eight Three Denied Directory\\key.txt"
    ));
    TEST_ASSERT(denied(
        &long_rule_compiled,
        SANDBOX_READ,
        short_allowed + "\\Eight Three Denied Directory\\new.txt"
    ));

    std::string short_denied;
    const std::string denied_rule = distinct_short_path_for_test(denied_root, &short_denied)
                                        ? short_denied
                                        : short_allowed + "\\Eight Three Denied Directory";
    TEST_ASSERT(denied(&long_rule_compiled, SANDBOX_READ, denied_rule + "\\key.txt"));
    TEST_ASSERT(denied(&long_rule_compiled, SANDBOX_READ, denied_rule + "\\new.txt"));

    FilesystemSandbox short_rule_sandbox;
    short_rule_sandbox.read.allow.push_back(short_allowed);
    short_rule_sandbox.read.deny.push_back(denied_rule);
    const CompiledFilesystemSandbox short_rule_compiled =
        compile_filesystem_sandbox(short_rule_sandbox);
    authorize_path(&short_rule_compiled, SANDBOX_READ, (public_root / "ok.txt").string());
    authorize_path(&short_rule_compiled, SANDBOX_READ, (public_root / "new.txt").string());
    TEST_ASSERT(denied(&short_rule_compiled, SANDBOX_READ, (denied_root / "key.txt").string()));
    TEST_ASSERT(denied(&short_rule_compiled, SANDBOX_READ, (denied_root / "new.txt").string()));
}
#endif

} // namespace

int main() {
#ifdef _WIN32
    test_concurrent_first_path_comparison();
#endif
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
#ifdef _WIN32
    const std::string short_allowed = short_path_for_test(allowed);
    if (!short_allowed.empty() && !host_path_equal(short_allowed, allowed.string())) {
        authorize_path(&compiled, SANDBOX_WRITE, short_allowed + "\\new-short-name.txt");
    }
#endif
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
    TEST_ASSERT(
        normalize_for_system(windows_policy, "/cygdrive/c/Work/file.txt") == "C:\\Work\\file.txt"
    );
    TEST_ASSERT(
        join_for_policy(windows_policy, "C:/Work", "nested/file.txt")
        == "C:\\Work\\nested\\file.txt"
    );
    std::string resolved_windows_posix_path;
    TEST_ASSERT(resolve_absolute_input_path_for_policy(
        windows_policy,
        "/usr/bin/bash",
        "C:/msys64",
        &resolved_windows_posix_path
    ));
    TEST_ASSERT(resolved_windows_posix_path == "C:\\msys64\\usr\\bin\\bash");
    TEST_ASSERT(resolve_absolute_input_path_for_policy(
        windows_policy,
        "/",
        "C:/msys64",
        &resolved_windows_posix_path
    ));
    TEST_ASSERT(resolved_windows_posix_path == "C:\\msys64");
    TEST_ASSERT(resolve_absolute_input_path_for_policy(
        windows_policy,
        "/c/Work/file.txt",
        "D:/ignored",
        &resolved_windows_posix_path
    ));
    TEST_ASSERT(resolved_windows_posix_path == "C:\\Work\\file.txt");
    TEST_ASSERT(resolve_absolute_input_path_for_policy(
        windows_policy,
        "//server/share/file.txt",
        "C:/msys64",
        &resolved_windows_posix_path
    ));
    TEST_ASSERT(resolved_windows_posix_path == "\\\\server\\share\\file.txt");

#ifdef _WIN32
    TEST_ASSERT(host_path_equal("/c/WORK/File.txt", "c:\\work\\file.txt"));
    assert_unicode_path_case_folding();
    TEST_ASSERT(host_path_is_within("C:\\WORK\\SECRET\\key.txt", "c:/work/secret"));
    TEST_ASSERT(!host_path_is_within("C:\\Worker\\out.txt", "c:/work"));

    FilesystemSandbox windows_sandbox;
    windows_sandbox.read.allow.push_back("C:\\Work");
    windows_sandbox.read.deny.push_back("C:\\Work\\Secret");
    const CompiledFilesystemSandbox windows_compiled = compile_filesystem_sandbox(windows_sandbox);
    authorize_path(&windows_compiled, SANDBOX_READ, "c:/work/out.txt");
    TEST_ASSERT(denied(&windows_compiled, SANDBOX_READ, "C:\\WORK\\SECRET\\key.txt"));
    TEST_ASSERT(denied(&windows_compiled, SANDBOX_READ, "C:\\Worker\\out.txt"));

    const std::wstring unicode_allow = L"C:\\R\u00C9SUM\u00C9";
    const std::wstring unicode_deny = L"C:\\R\u00C9SUM\u00C9\\\u00C4rger";
    const std::wstring unicode_authorized = L"c:/r\u00E9sum\u00E9/out.txt";
    const std::wstring unicode_denied = L"C:\\r\u00E9sum\u00E9\\\u00E4rger\\key.txt";
    const std::wstring unicode_paths[] = {
        unicode_allow,
        unicode_deny,
        unicode_authorized,
        unicode_denied,
    };
    if (ansi_represents_paths(unicode_paths, 4U)) {
        FilesystemSandbox unicode_windows_sandbox;
        unicode_windows_sandbox.read.allow.push_back(win32_utf8::utf8_from_wide(unicode_allow));
        unicode_windows_sandbox.read.deny.push_back(win32_utf8::utf8_from_wide(unicode_deny));
        const CompiledFilesystemSandbox unicode_windows_compiled =
            compile_filesystem_sandbox(unicode_windows_sandbox);
        authorize_path(
            &unicode_windows_compiled,
            SANDBOX_READ,
            win32_utf8::utf8_from_wide(unicode_authorized)
        );
        TEST_ASSERT(denied(
            &unicode_windows_compiled,
            SANDBOX_READ,
            win32_utf8::utf8_from_wide(unicode_denied)
        ));
    }

    run_windows_short_alias_sandbox_cases();
#endif

    run_shared_path_policy_cases();
    run_shared_path_compare_cases();
    run_shared_sandbox_cases();

    return 0;
}
