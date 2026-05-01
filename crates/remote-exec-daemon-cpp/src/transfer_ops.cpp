#include <stdexcept>
#include <string>

#include "transfer_ops.h"
#include "transfer_ops_internal.h"

using namespace transfer_ops_internal;

PathInfo path_info(const std::string& absolute_path) {
    if (!is_absolute_path(absolute_path)) {
        throw std::runtime_error("transfer path is not absolute");
    }
    if (!path_exists(absolute_path)) {
        return PathInfo{false, false};
    }
    if (is_symlink_path(absolute_path)) {
        throw std::runtime_error("destination path contains unsupported symlink");
    }
    return PathInfo{true, is_directory(absolute_path)};
}
