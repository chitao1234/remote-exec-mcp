#include <algorithm>
#include <cctype>
#include <string>

#include "path_policy.h"

namespace {

bool is_ascii_alpha(char ch) {
    return std::isalpha(static_cast<unsigned char>(ch)) != 0;
}

std::string normalize_windows_path_chars(std::string value) {
    std::replace(value.begin(), value.end(), '/', '\\');
    return value;
}

std::string build_windows_drive_path(char drive, const std::string& rest) {
    std::string tail = rest;
    while (!tail.empty() && (tail[0] == '/' || tail[0] == '\\')) {
        tail.erase(0, 1);
    }

    std::string output;
    output.push_back(static_cast<char>(std::toupper(static_cast<unsigned char>(drive))));
    output += ":\\";
    if (!tail.empty()) {
        output += normalize_windows_path_chars(tail);
    }
    return output;
}

bool translate_windows_posix_drive_path(const std::string& raw, std::string* output) {
    if (raw.size() >= 2 && raw[0] == '/' && is_ascii_alpha(raw[1]) &&
        (raw.size() == 2 || raw[2] == '/')) {
        *output = build_windows_drive_path(raw[1], raw.substr(raw.size() == 2 ? 2 : 3));
        return true;
    }

    const std::string prefix = "/cygdrive/";
    const std::string lower = path_policy_lowercase_ascii(raw);
    if (lower.rfind(prefix, 0) != 0) {
        return false;
    }
    if (raw.size() <= prefix.size() || !is_ascii_alpha(raw[prefix.size()])) {
        return false;
    }
    if (raw.size() > prefix.size() + 1 && raw[prefix.size() + 1] != '/') {
        return false;
    }

    *output = build_windows_drive_path(
        raw[prefix.size()],
        raw.substr(raw.size() == prefix.size() + 1 ? prefix.size() + 1 : prefix.size() + 2)
    );
    return true;
}

}  // namespace

PathPolicy posix_path_policy() {
    PathPolicy policy;
    policy.style = PATH_STYLE_POSIX;
    policy.comparison = PATH_COMPARISON_CASE_SENSITIVE;
    return policy;
}

PathPolicy windows_path_policy() {
    PathPolicy policy;
    policy.style = PATH_STYLE_WINDOWS;
    policy.comparison = PATH_COMPARISON_CASE_INSENSITIVE;
    return policy;
}

PathPolicy host_path_policy() {
#ifdef _WIN32
    return windows_path_policy();
#else
    return posix_path_policy();
#endif
}

std::string path_policy_lowercase_ascii(std::string value) {
    for (std::size_t i = 0; i < value.size(); ++i) {
        value[i] = static_cast<char>(std::tolower(static_cast<unsigned char>(value[i])));
    }
    return value;
}

std::string path_policy_comparison_key(PathPolicy policy, const std::string& raw) {
    std::string normalized = normalize_for_system(policy, raw);
    if (policy.comparison == PATH_COMPARISON_CASE_INSENSITIVE) {
        normalized = path_policy_lowercase_ascii(normalized);
    }
    return normalized;
}

bool is_absolute_for_policy(PathPolicy policy, const std::string& raw) {
    if (policy.style == PATH_STYLE_POSIX) {
        return !raw.empty() && raw[0] == '/';
    }

    if (raw.size() >= 3 && is_ascii_alpha(raw[0]) && raw[1] == ':' &&
        (raw[2] == '\\' || raw[2] == '/')) {
        return true;
    }
    if (raw.rfind("\\\\", 0) == 0 || raw.rfind("//", 0) == 0) {
        return true;
    }

    std::string translated;
    return translate_windows_posix_drive_path(raw, &translated);
}

std::string normalize_for_system(PathPolicy policy, const std::string& raw) {
    if (policy.style == PATH_STYLE_POSIX) {
        return raw;
    }

    std::string translated;
    if (translate_windows_posix_drive_path(raw, &translated)) {
        return translated;
    }
    return normalize_windows_path_chars(raw);
}

std::string join_for_policy(
    PathPolicy policy,
    const std::string& base,
    const std::string& child
) {
    const std::string normalized_child = normalize_for_system(policy, child);
    if (normalized_child.empty() || is_absolute_for_policy(policy, normalized_child)) {
        return normalized_child;
    }

    const std::string normalized_base = normalize_for_system(policy, base);
    if (normalized_base.empty()) {
        return normalized_child;
    }

    const char separator = policy.style == PATH_STYLE_WINDOWS ? '\\' : '/';
    if (normalized_base[normalized_base.size() - 1] == separator) {
        return normalized_base + normalized_child;
    }
    return normalized_base + separator + normalized_child;
}

bool same_path_for_policy(
    PathPolicy policy,
    const std::string& left,
    const std::string& right
) {
    return path_policy_comparison_key(policy, left) == path_policy_comparison_key(policy, right);
}
