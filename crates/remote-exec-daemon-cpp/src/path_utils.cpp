#include "path_utils.h"

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
    if (base.empty()) {
        return child;
    }
    std::string joined = base;
    if (joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(native_separator());
    }
    joined += child;
    return joined;
}

}  // namespace path_utils
