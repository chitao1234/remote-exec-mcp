#include "path_utils.h"

#include <algorithm>

namespace path_utils {

char native_separator() {
#ifdef _WIN32
    return '\\';
#else
    return '/';
#endif
}

std::string parent_directory(const std::string& path) {
    const std::size_t slash = path.find_last_of("/\\");
    if (slash == std::string::npos) {
        return "";
    }
    return path.substr(0, slash);
}

std::string join_path(const std::string& base, const std::string& child) {
    std::string normalized_child = child;
#ifdef _WIN32
    std::replace(normalized_child.begin(), normalized_child.end(), '/', '\\');
#endif
    if (base.empty()) {
        return normalized_child;
    }
    std::string joined = base;
#ifdef _WIN32
    std::replace(joined.begin(), joined.end(), '/', '\\');
#endif
    if (joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(native_separator());
    }
    joined += normalized_child;
    return joined;
}

}  // namespace path_utils
