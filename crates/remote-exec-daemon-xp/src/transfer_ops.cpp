#include <cerrno>
#include <cstdio>
#include <fstream>
#include <sstream>
#include <stdexcept>

#ifdef _WIN32
#include <direct.h>
#include <sys/stat.h>
#else
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "transfer_ops.h"

namespace {

bool is_absolute_path(const std::string& path) {
#ifdef _WIN32
    return (path.size() >= 3 && std::isalpha(static_cast<unsigned char>(path[0])) != 0 &&
            path[1] == ':' && (path[2] == '\\' || path[2] == '/')) ||
           path.rfind("\\\\", 0) == 0 || path.rfind("//", 0) == 0;
#else
    return !path.empty() && path[0] == '/';
#endif
}

std::string parent_directory(const std::string& path) {
    const std::size_t slash = path.find_last_of("/\\");
    if (slash == std::string::npos) {
        return "";
    }
    return path.substr(0, slash);
}

bool stat_path(const std::string& path, struct stat* st) {
    return stat(path.c_str(), st) == 0;
}

bool is_regular_file(const std::string& path) {
    struct stat st;
    return stat_path(path, &st) && (st.st_mode & S_IFREG) != 0;
}

bool is_directory(const std::string& path) {
    struct stat st;
    return stat_path(path, &st) && (st.st_mode & S_IFDIR) != 0;
}

void make_directory_if_missing(const std::string& path) {
    if (path.empty() || is_directory(path)) {
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

void ensure_parent_directory(const std::string& path, bool create_parent) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }
    if (!create_parent) {
        if (!is_directory(parent)) {
            throw std::runtime_error("destination parent does not exist");
        }
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

std::string read_binary_file(const std::string& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    if (!input) {
        throw std::runtime_error("transfer source missing");
    }
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

} // namespace

ExportedFile export_file(const std::string& absolute_path) {
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }
    if (!is_regular_file(absolute_path)) {
        throw std::runtime_error("transfer source must be a regular file");
    }

    return ExportedFile{"file", read_binary_file(absolute_path)};
}

ImportSummary import_file(
    const std::string& bytes,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
) {
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }

    const bool existed = is_regular_file(absolute_path);
    if (existed && !replace_existing) {
        throw std::runtime_error("destination path already exists");
    }

    ensure_parent_directory(absolute_path, create_parent);

    std::ofstream output(absolute_path.c_str(), std::ios::binary | std::ios::trunc);
    if (!output) {
        throw std::runtime_error("unable to write destination file");
    }
    output.write(bytes.data(), static_cast<std::streamsize>(bytes.size()));

    return ImportSummary{
        "file",
        static_cast<std::uint64_t>(bytes.size()),
        1,
        0,
        existed,
    };
}
