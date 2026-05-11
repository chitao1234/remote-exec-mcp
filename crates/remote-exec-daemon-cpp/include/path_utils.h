#ifndef REMOTE_EXEC_PATH_UTILS_H
#define REMOTE_EXEC_PATH_UTILS_H

#include <string>

namespace path_utils {

char native_separator();
std::string parent_directory(const std::string& path);
std::string join_path(const std::string& base, const std::string& child);
void make_directory_if_missing(const std::string& path);
void create_parent_directories(const std::string& path);

}  // namespace path_utils

#endif
