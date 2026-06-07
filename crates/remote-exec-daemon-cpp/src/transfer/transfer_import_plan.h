#pragma once

#include <string>
#include <vector>

namespace transfer_import_plan {

std::vector<std::string> split_archive_path(const std::string& path);
std::string validate_relative_archive_path(const std::string& raw_path);
std::string validate_relative_symlink_target(const std::string& raw_target);
std::string materialize_archive_path(
    const std::string& destination_root,
    const std::string& relative_archive_path
);
std::string top_level_archive_component(const std::string& relative_archive_path);
std::string resolved_symlink_target_path(
    const std::string& symlink_path,
    const std::string& relative_target
);

} // namespace transfer_import_plan
