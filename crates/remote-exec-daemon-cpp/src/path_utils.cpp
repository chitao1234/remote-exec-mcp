#include "path_utils.h"

#include <algorithm>
#include <cerrno>
#include <stdexcept>

#ifdef _WIN32
#include <direct.h>
#else
#include <sys/stat.h>
#include <sys/types.h>
#endif

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

void make_directory_if_missing(const std::string& path) {
    if (path.empty()) {
        return;
    }
#ifdef _WIN32
    if (_mkdir(path.c_str()) != 0 && errno != EEXIST) {
#else
    if (mkdir(path.c_str(), 0777) != 0 && errno != EEXIST) {
#endif
        throw std::runtime_error("unable to create directory " + path);
    }
}

void create_parent_directories(const std::string& path) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }

    std::string current;
    for (std::size_t i = 0; i < parent.size(); ++i) {
        const char ch = parent[i];
        current.push_back(ch);
        if (ch != '/' && ch != '\\') {
            continue;
        }
        if (current.size() == 1) {
            continue;
        }
        if (current.size() == 3 && current[1] == ':') {
            continue;
        }
        current.erase(current.size() - 1);
        make_directory_if_missing(current);
        current.push_back(ch);
    }
    make_directory_if_missing(parent);
}

} // namespace path_utils
