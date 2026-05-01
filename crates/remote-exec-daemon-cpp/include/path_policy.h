#pragma once

#include <string>

enum PathStyle {
    PATH_STYLE_POSIX,
    PATH_STYLE_WINDOWS,
};

enum PathComparison {
    PATH_COMPARISON_CASE_SENSITIVE,
    PATH_COMPARISON_CASE_INSENSITIVE,
};

struct PathPolicy {
    PathStyle style;
    PathComparison comparison;
};

PathPolicy posix_path_policy();
PathPolicy windows_path_policy();
PathPolicy host_path_policy();

bool is_absolute_for_policy(PathPolicy policy, const std::string& raw);
std::string normalize_for_system(PathPolicy policy, const std::string& raw);
std::string join_for_policy(
    PathPolicy policy,
    const std::string& base,
    const std::string& child
);
bool same_path_for_policy(
    PathPolicy policy,
    const std::string& left,
    const std::string& right
);
