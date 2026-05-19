#pragma once

#include <string>

enum PathStyle {
    PATH_STYLE_POSIX,
    PATH_STYLE_WINDOWS,
};

struct PathPolicy {
    PathStyle style;
};

PathPolicy posix_path_policy();
PathPolicy windows_path_policy();
PathPolicy host_path_policy();

bool is_absolute_for_policy(PathPolicy policy, const std::string& raw);
std::string normalize_for_system(PathPolicy policy, const std::string& raw);
bool syntax_eq_for_policy(PathPolicy policy, const std::string& left, const std::string& right);
std::string join_for_policy(PathPolicy policy, const std::string& base, const std::string& child);
bool basename_for_policy(PathPolicy policy, const std::string& raw, std::string* basename);
