#pragma once

#include <stdexcept>
#include <string>

#include "http/http_helpers.h"
#include "test_assert.h"
#include "test_filesystem.h"

#ifdef _WIN32
#include <direct.h>
#else
#include <sys/stat.h>
#include <unistd.h>
#endif

namespace test_contract {

inline std::string replace_all(
    std::string value,
    const std::string& needle,
    const std::string& replacement
) {
    std::string::size_type position = 0;
    while ((position = value.find(needle, position)) != std::string::npos) {
        value.replace(position, needle.size(), replacement);
        position += replacement.size();
    }
    return value;
}

inline std::string apply_template(const std::string& raw, const test_fs::path& root) {
    return replace_all(raw, "{root}", root.string());
}

inline bool case_applies_to_host(const Json& case_json) {
    if (!case_json.contains("platforms")) {
        return true;
    }
#ifdef _WIN32
    const std::string platform = "windows";
#else
    const std::string platform = "posix";
#endif
    const Json& platforms = case_json.at("platforms");
    for (Json::const_iterator it = platforms.begin(); it != platforms.end(); ++it) {
        if (it->get<std::string>() == platform) {
            return true;
        }
    }
    return false;
}

inline void apply_setup(const test_fs::path& root, const Json& setup) {
    if (setup.is_null()) {
        return;
    }

    if (setup.contains("dirs")) {
        const Json& dirs = setup.at("dirs");
        for (Json::const_iterator it = dirs.begin(); it != dirs.end(); ++it) {
            test_fs::create_directories(apply_template(it->get<std::string>(), root));
        }
    }

    if (setup.contains("files")) {
        const Json& files = setup.at("files");
        for (Json::const_iterator it = files.begin(); it != files.end(); ++it) {
            const test_fs::path path = apply_template(it->at("path").get<std::string>(), root);
            test_fs::create_directories(path.parent_path());
            test_fs::write_file_bytes(path, it->at("contents").get<std::string>());
        }
    }

#ifndef _WIN32
    if (setup.contains("symlinks")) {
        const Json& symlinks = setup.at("symlinks");
        for (Json::const_iterator it = symlinks.begin(); it != symlinks.end(); ++it) {
            const test_fs::path path = apply_template(it->at("path").get<std::string>(), root);
            test_fs::create_directories(path.parent_path());
            test_fs::create_symlink(
                apply_template(it->at("target").get<std::string>(), root),
                path
            );
        }
    }

    if (setup.contains("fifos")) {
        const Json& fifos = setup.at("fifos");
        for (Json::const_iterator it = fifos.begin(); it != fifos.end(); ++it) {
            const test_fs::path path = apply_template(it->get<std::string>(), root);
            test_fs::create_directories(path.parent_path());
            TEST_ASSERT(mkfifo(path.c_str(), 0600) == 0);
        }
    }
#endif
}

inline test_fs::path current_working_directory() {
    char buffer[4096];
#ifdef _WIN32
    if (_getcwd(buffer, static_cast<int>(sizeof(buffer))) == NULL) {
        throw std::runtime_error("unable to determine current working directory");
    }
#else
    if (getcwd(buffer, sizeof(buffer)) == NULL) {
        throw std::runtime_error("unable to determine current working directory");
    }
#endif
    return test_fs::path(buffer);
}

inline test_fs::path contract_fixture_root() {
    test_fs::path current = current_working_directory();
    for (int depth = 0; depth < 8; ++depth) {
        const test_fs::path candidate = current / "tests" / "contracts";
        if (test_fs::is_directory(candidate)) {
            return candidate;
        }
        const test_fs::path parent = current.parent_path();
        if (parent == current || parent.string().empty()) {
            break;
        }
        current = parent;
    }
    throw std::runtime_error("unable to locate tests/contracts fixture directory");
}

inline Json load_contract_fixture(const std::string& relative_path) {
    return Json::parse(test_fs::read_file_bytes(contract_fixture_root() / relative_path));
}

inline const Json& port_tunnel_contract() {
    static const Json fixture = load_contract_fixture("port_tunnel/contract.json");
    return fixture;
}

inline const Json& transfer_headers_contract() {
    static const Json fixture = load_contract_fixture("transfer_headers/contract.json");
    return fixture;
}

inline const Json& path_policy_cases() {
    static const Json fixture = load_contract_fixture("path_policy_cases.json");
    return fixture;
}

inline const Json& path_compare_cases() {
    static const Json fixture = load_contract_fixture("path_compare_cases.json");
    return fixture;
}

inline const Json& sandbox_cases() {
    static const Json fixture = load_contract_fixture("sandbox_cases.json");
    return fixture;
}

inline const Json& transfer_semantics_contract() {
    static const Json fixture = load_contract_fixture("transfer_semantics/contract.json");
    return fixture;
}

} // namespace test_contract
