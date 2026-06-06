#include <cerrno>
#include <stdexcept>
#include <string>

#include "platform/path_utils.h"
#include "platform/platform.h"
#include "rpc/rpc_failures.h"
#include "transfer/transfer_ops.h"
#include "transfer_filesystem.h"

TransferArchiveReader::~TransferArchiveReader() {
}

TransferArchiveSink::~TransferArchiveSink() {
}

void TransferArchiveSink::write_string(const std::string& data) {
    write(data.data(), data.size());
}

const char* transfer_source_type_wire_value(TransferSourceType source_type) {
    switch (source_type) {
    case TransferSourceType::File:
        return "file";
    case TransferSourceType::Directory:
        return "directory";
    case TransferSourceType::Multiple:
        return "multiple";
    }
    return "file";
}

bool parse_transfer_source_type_wire_value(const std::string& value, TransferSourceType* source_type) {
    if (value == "file") {
        *source_type = TransferSourceType::File;
        return true;
    }
    if (value == "directory") {
        *source_type = TransferSourceType::Directory;
        return true;
    }
    if (value == "multiple") {
        *source_type = TransferSourceType::Multiple;
        return true;
    }
    return false;
}

const char* transfer_symlink_mode_wire_value(TransferSymlinkMode symlink_mode) {
    switch (symlink_mode) {
    case TransferSymlinkMode::Preserve:
        return "preserve";
    case TransferSymlinkMode::Follow:
        return "follow";
    case TransferSymlinkMode::Skip:
        return "skip";
    }
    return "preserve";
}

bool parse_transfer_symlink_mode_wire_value(const std::string& value, TransferSymlinkMode* symlink_mode) {
    if (value == "preserve") {
        *symlink_mode = TransferSymlinkMode::Preserve;
        return true;
    }
    if (value == "follow") {
        *symlink_mode = TransferSymlinkMode::Follow;
        return true;
    }
    if (value == "skip") {
        *symlink_mode = TransferSymlinkMode::Skip;
        return true;
    }
    return false;
}

const char* transfer_overwrite_wire_value(TransferOverwrite overwrite) {
    switch (overwrite) {
    case TransferOverwrite::Fail:
        return "fail";
    case TransferOverwrite::Merge:
        return "merge";
    case TransferOverwrite::Replace:
        return "replace";
    }
    return "fail";
}

bool parse_transfer_overwrite_wire_value(const std::string& value, TransferOverwrite* overwrite) {
    if (value == "fail") {
        *overwrite = TransferOverwrite::Fail;
        return true;
    }
    if (value == "merge") {
        *overwrite = TransferOverwrite::Merge;
        return true;
    }
    if (value == "replace") {
        *overwrite = TransferOverwrite::Replace;
        return true;
    }
    return false;
}

PathInfo path_info(const std::string& absolute_path) {
    if (!transfer_filesystem::is_absolute_path(absolute_path)) {
        throw TransferFailure(TransferRpcCode::PathNotAbsolute, "transfer path is not absolute");
    }

    path_utils::PathMetadata metadata;
    if (!path_utils::path_metadata_no_follow(absolute_path, &metadata)) {
        const int error_code = errno;
        if (error_code == ENOENT || error_code == ENOTDIR) {
            return PathInfo{false, false};
        }
        throw TransferFailure(TransferRpcCode::Internal, errno_error::message_from_errno(error_code));
    }

    if (metadata.is_symlink) {
        throw TransferFailure(TransferRpcCode::DestinationUnsupported, "destination path contains unsupported symlink");
    }
    return PathInfo{true, metadata.is_directory};
}
