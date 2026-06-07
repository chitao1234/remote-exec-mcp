#include <cerrno>
#include <string>

#include "platform/path_utils.h"
#include "platform/platform.h"
#include "rpc/rpc_failures.h"
#include "transfer/transfer_ops.h"
#include "transfer_filesystem.h"

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
        throw TransferFailure(
            TransferRpcCode::Internal,
            errno_error::message_from_errno(error_code)
        );
    }

    if (metadata.is_symlink) {
        throw TransferFailure(
            TransferRpcCode::DestinationUnsupported,
            "destination path contains unsupported symlink"
        );
    }
    return PathInfo{true, metadata.is_directory};
}
