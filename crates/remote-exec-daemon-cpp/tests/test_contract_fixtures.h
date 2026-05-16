#pragma once

#include <stdexcept>
#include <string>

#include "http_helpers.h"
#include "test_filesystem.h"

#ifdef _WIN32
#include <direct.h>
#else
#include <unistd.h>
#endif

namespace test_contract {

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

} // namespace test_contract
