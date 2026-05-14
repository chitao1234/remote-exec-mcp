#pragma once

#include <string>

bool host_path_equal(const std::string& left, const std::string& right);
bool host_path_has_prefix(const std::string& path, const std::string& prefix);
bool host_path_is_within(const std::string& path, const std::string& root);
