#include <algorithm>
#include <cctype>

#include "platform/path_utils.h"
#include "rpc/rpc_failures.h"
#include "transfer_filesystem.h"
#include "transfer_import_plan.h"

namespace transfer_import_plan {

namespace {

std::string trim_trailing_slashes(std::string value) {
    while (value.size() > 1 && !value.empty() && value[value.size() - 1] == '/') {
        value.erase(value.size() - 1);
    }
    return value;
}

std::string normalize_archive_separators(std::string value) {
    std::replace(value.begin(), value.end(), '\\', '/');
    return value;
}

} // namespace

std::vector<std::string> split_archive_path(const std::string& path) {
    std::vector<std::string> parts;
    std::string current;
    for (std::size_t i = 0; i < path.size(); ++i) {
        const char ch = path[i];
        if (ch == '/') {
            parts.push_back(current);
            current.clear();
            continue;
        }
        current.push_back(ch);
    }
    parts.push_back(current);
    return parts;
}

std::string validate_relative_archive_path(const std::string& raw_path) {
    std::string normalized = normalize_archive_separators(raw_path);
    while (normalized.rfind("./", 0) == 0) {
        normalized.erase(0, 2);
    }
    normalized = trim_trailing_slashes(normalized);

    if (normalized.empty() || normalized == ".") {
        return ".";
    }
    if (normalized[0] == '/') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path must be relative");
    }
    if (normalized.size() >= 2 && std::isalpha(static_cast<unsigned char>(normalized[0])) != 0
        && normalized[1] == ':') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path must be relative");
    }
    if (normalized.rfind("//", 0) == 0) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive path must be relative");
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    std::vector<std::string> cleaned;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        const std::string& part = parts[i];
        if (part.empty()) {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive path contains empty component"
            );
        }
        if (part == "." || part == "..") {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive path escapes destination"
            );
        }
        cleaned.push_back(part);
    }

    std::string result;
    for (std::size_t i = 0; i < cleaned.size(); ++i) {
        if (i != 0) {
            result.push_back('/');
        }
        result += cleaned[i];
    }
    return result;
}

std::string validate_relative_symlink_target(const std::string& raw_target) {
    const std::string normalized = normalize_archive_separators(raw_target);
    if (normalized.empty() || normalized[0] == '/' || normalized.rfind("//", 0) == 0) {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "archive symlink target must be relative"
        );
    }
    if (normalized.size() >= 2 && std::isalpha(static_cast<unsigned char>(normalized[0])) != 0
        && normalized[1] == ':') {
        throw TransferFailure(
            TransferRpcCode::SourceUnsupported,
            "archive symlink target must be relative"
        );
    }

    const std::vector<std::string> parts = split_archive_path(normalized);
    for (std::size_t i = 0; i < parts.size(); ++i) {
        if (parts[i].empty() || parts[i] == "." || parts[i] == "..") {
            throw TransferFailure(
                TransferRpcCode::SourceUnsupported,
                "archive symlink target escapes destination"
            );
        }
    }
    return normalized;
}

std::string materialize_archive_path(
    const std::string& destination_root,
    const std::string& relative_archive_path
) {
    if (relative_archive_path == ".") {
        return destination_root;
    }

    const std::vector<std::string> parts = split_archive_path(relative_archive_path);
    std::string path = destination_root;
    for (std::size_t i = 0; i < parts.size(); ++i) {
        path = transfer_filesystem::join_path(path, parts[i]);
    }
    return path;
}

std::string top_level_archive_component(const std::string& relative_archive_path) {
    if (relative_archive_path == ".") {
        return "";
    }

    const std::vector<std::string> parts = split_archive_path(relative_archive_path);
    return parts.empty() ? "" : parts[0];
}

std::string resolved_symlink_target_path(
    const std::string& symlink_path,
    const std::string& relative_target
) {
    const std::string parent = path_utils::parent_directory(symlink_path);
    if (parent.empty()) {
        return relative_target;
    }
    return transfer_filesystem::join_path(parent, relative_target);
}

} // namespace transfer_import_plan
