#include <algorithm>
#include <cctype>
#include <cerrno>
#include <cstdlib>
#include <cstring>
#include <sstream>
#include <string>
#include <vector>

#ifndef _WIN32
#include <unistd.h>
#endif

#include "filesystem_sandbox.h"
#include "path_compare.h"

namespace {

const char* access_label(SandboxAccess access) {
    switch (access) {
    case SANDBOX_EXEC_CWD:
        return "exec_cwd";
    case SANDBOX_READ:
        return "read";
    case SANDBOX_WRITE:
        return "write";
    }
    return "unknown";
}

const SandboxPathList& source_list(const FilesystemSandbox& sandbox, SandboxAccess access) {
    switch (access) {
    case SANDBOX_EXEC_CWD:
        return sandbox.exec_cwd;
    case SANDBOX_READ:
        return sandbox.read;
    case SANDBOX_WRITE:
        return sandbox.write;
    }
    return sandbox.read;
}

const CompiledSandboxPathList& compiled_list(const CompiledFilesystemSandbox& sandbox, SandboxAccess access) {
    switch (access) {
    case SANDBOX_EXEC_CWD:
        return sandbox.exec_cwd;
    case SANDBOX_READ:
        return sandbox.read;
    case SANDBOX_WRITE:
        return sandbox.write;
    }
    return sandbox.read;
}

bool is_separator(PathPolicy policy, char ch) {
    if (policy.style == PATH_STYLE_WINDOWS) {
        return ch == '\\' || ch == '/';
    }
    return ch == '/';
}

char policy_separator(PathPolicy policy) {
    return policy.style == PATH_STYLE_WINDOWS ? '\\' : '/';
}

std::string join_components(PathPolicy policy, const std::string& prefix, const std::vector<std::string>& parts) {
    const char separator = policy_separator(policy);
    if (prefix.empty()) {
        std::string output;
        for (std::size_t i = 0; i < parts.size(); ++i) {
            if (i != 0) {
                output.push_back(separator);
            }
            output += parts[i];
        }
        return output;
    }

    std::string output = prefix;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        if (output.empty() || output[output.size() - 1] != separator) {
            output.push_back(separator);
        }
        output += parts[i];
    }
    return output;
}

std::string lexical_normalize_for_policy(PathPolicy policy, const std::string& raw) {
    const std::string normalized = normalize_for_system(policy, raw);
    std::string prefix;
    std::size_t start = 0;

    if (policy.style == PATH_STYLE_POSIX) {
        if (!normalized.empty() && normalized[0] == '/') {
            prefix = "/";
            start = 1;
        }
    } else if (normalized.size() >= 3 && std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
               normalized[1] == ':' && is_separator(policy, normalized[2])) {
        prefix = normalized.substr(0, 2);
        prefix.push_back('\\');
        start = 3;
    } else if (normalized.rfind("\\\\", 0) == 0) {
        prefix = "\\\\";
        start = 2;
    }

    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = start; i < normalized.size(); ++i) {
        const char ch = normalized[i];
        if (is_separator(policy, ch)) {
            if (current.empty()) {
                continue;
            }
            if (current == ".") {
                current.clear();
                continue;
            }
            if (current == "..") {
                if (!parts.empty()) {
                    parts.pop_back();
                }
                current.clear();
                continue;
            }
            parts.push_back(current);
            current.clear();
            continue;
        }
        current.push_back(ch);
    }

    if (!current.empty() && current != ".") {
        if (current == "..") {
            if (!parts.empty()) {
                parts.pop_back();
            }
        } else {
            parts.push_back(current);
        }
    }

    const std::string output = join_components(policy, prefix, parts);
    if (output.empty() && !prefix.empty()) {
        return prefix;
    }
    return output;
}

#ifndef _WIN32
std::string basename_for_posix_path(std::string path) {
    while (path.size() > 1 && path[path.size() - 1] == '/') {
        path.erase(path.size() - 1);
    }
    if (path == "/") {
        return "";
    }
    const std::size_t slash = path.find_last_of('/');
    if (slash == std::string::npos) {
        return path;
    }
    return path.substr(slash + 1);
}

std::string parent_for_posix_path(std::string path) {
    while (path.size() > 1 && path[path.size() - 1] == '/') {
        path.erase(path.size() - 1);
    }
    const std::size_t slash = path.find_last_of('/');
    if (slash == std::string::npos) {
        return "";
    }
    if (slash == 0) {
        return "/";
    }
    return path.substr(0, slash);
}

std::string canonicalize_posix_for_sandbox(const std::string& path) {
    const PathPolicy policy = posix_path_policy();
    const std::string normalized = lexical_normalize_for_policy(policy, path);
    std::string probe = normalized;
    std::vector<std::string> missing_components;

    for (;;) {
        errno = 0;
        char* resolved = realpath(probe.c_str(), nullptr);
        if (resolved != nullptr) {
            std::string rebuilt(resolved);
            std::free(resolved);
            rebuilt = lexical_normalize_for_policy(policy, rebuilt);
            for (std::vector<std::string>::const_reverse_iterator it = missing_components.rbegin();
                 it != missing_components.rend();
                 ++it) {
                rebuilt = join_for_policy(policy, rebuilt, *it);
            }
            return lexical_normalize_for_policy(policy, rebuilt);
        }

        if (errno != ENOENT) {
            throw SandboxError("unable to canonicalize `" + normalized + "`: " + std::strerror(errno));
        }

        const std::string name = basename_for_posix_path(probe);
        if (name.empty()) {
            throw SandboxError("unable to resolve an existing ancestor for `" + normalized + "`");
        }
        missing_components.push_back(name);

        const std::string parent = parent_for_posix_path(probe);
        if (parent.empty() || parent == probe) {
            throw SandboxError("unable to resolve an existing ancestor for `" + normalized + "`");
        }
        probe = parent;
    }
}
#endif

std::string canonicalize_for_sandbox(const std::string& path) {
    const PathPolicy policy = host_path_policy();
    if (policy.style == PATH_STYLE_WINDOWS) {
        return lexical_normalize_for_policy(policy, path);
    }

#ifndef _WIN32
    return canonicalize_posix_for_sandbox(path);
#else
    return lexical_normalize_for_policy(policy, path);
#endif
}

std::string compile_root(SandboxAccess access, const std::string& list_label, const std::string& raw) {
    const PathPolicy policy = host_path_policy();
    if (!is_absolute_for_policy(policy, raw)) {
        throw SandboxError(std::string("sandbox ") + access_label(access) + "." + list_label + " path `" + raw +
                           "` is not absolute");
    }

    const std::string normalized = normalize_for_system(policy, raw);
    try {
        return canonicalize_for_sandbox(normalized);
    } catch (const SandboxError& ex) {
        throw SandboxError(std::string("sandbox ") + access_label(access) + "." + list_label + " path `" + normalized +
                           "` is invalid: " + ex.what());
    }
}

CompiledSandboxPathList compile_list(SandboxAccess access, const SandboxPathList& list) {
    CompiledSandboxPathList compiled;
    for (std::size_t i = 0; i < list.allow.size(); ++i) {
        compiled.allow.push_back(compile_root(access, "allow", list.allow[i]));
    }
    for (std::size_t i = 0; i < list.deny.size(); ++i) {
        compiled.deny.push_back(compile_root(access, "deny", list.deny[i]));
    }
    return compiled;
}

} // namespace

SandboxError::SandboxError(const std::string& message) : std::runtime_error(message) {
}

CompiledFilesystemSandbox compile_filesystem_sandbox(const FilesystemSandbox& sandbox) {
    CompiledFilesystemSandbox compiled;
    compiled.exec_cwd = compile_list(SANDBOX_EXEC_CWD, source_list(sandbox, SANDBOX_EXEC_CWD));
    compiled.read = compile_list(SANDBOX_READ, source_list(sandbox, SANDBOX_READ));
    compiled.write = compile_list(SANDBOX_WRITE, source_list(sandbox, SANDBOX_WRITE));
    return compiled;
}

void authorize_path(const CompiledFilesystemSandbox* sandbox, SandboxAccess access, const std::string& path) {
    const PathPolicy policy = host_path_policy();
    if (!is_absolute_for_policy(policy, path)) {
        throw SandboxError(std::string("path `") + path + "` is not absolute");
    }

    if (sandbox == nullptr) {
        return;
    }

    const std::string resolved = canonicalize_for_sandbox(path);
    const CompiledSandboxPathList& rules = compiled_list(*sandbox, access);

    for (std::size_t i = 0; i < rules.deny.size(); ++i) {
        if (host_path_is_within(resolved, rules.deny[i])) {
            throw SandboxError(std::string(access_label(access)) + " access to `" + resolved +
                               "` is denied by sandbox rule `" + rules.deny[i] + "`");
        }
    }

    if (rules.allow.empty()) {
        return;
    }
    for (std::size_t i = 0; i < rules.allow.size(); ++i) {
        if (host_path_is_within(resolved, rules.allow[i])) {
            return;
        }
    }

    throw SandboxError(std::string(access_label(access)) + " access to `" + resolved +
                       "` is outside the configured sandbox");
}
