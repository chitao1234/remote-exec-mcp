#ifndef REMOTE_EXEC_PATH_UTILS_H
#define REMOTE_EXEC_PATH_UTILS_H

#include <string>

namespace path_utils {

char native_separator();
std::string parent_directory(const std::string& path);
std::string join_path(const std::string& base, const std::string& child);

}  // namespace path_utils

#endif
