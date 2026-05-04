#include <cerrno>
#include <cstring>
#include <stdexcept>
#include <string>

#ifdef _WIN32
#include <sys/stat.h>
#else
#include <sys/stat.h>
#include <unistd.h>
#endif

#include "rpc_failures.h"
#include "transfer_ops.h"
#include "transfer_ops_internal.h"

using namespace transfer_ops_internal;

TransferArchiveReader::~TransferArchiveReader() {}

TransferArchiveSink::~TransferArchiveSink() {}

void TransferArchiveSink::write_string(const std::string& data) {
    write(data.data(), data.size());
}

PathInfo path_info(const std::string& absolute_path) {
    if (!is_absolute_path(absolute_path)) {
        throw TransferFailure(
            TransferRpcCode::PathNotAbsolute,
            "transfer path is not absolute"
        );
    }

    struct stat st;
#ifdef _WIN32
    if (stat(absolute_path.c_str(), &st) != 0) {
#else
    if (lstat(absolute_path.c_str(), &st) != 0) {
#endif
        const int error_code = errno;
        if (error_code == ENOENT || error_code == ENOTDIR) {
            return PathInfo{false, false};
        }
        throw TransferFailure(TransferRpcCode::Internal, std::strerror(error_code));
    }

#ifdef _WIN32
    return PathInfo{true, (st.st_mode & S_IFMT) == S_IFDIR};
#else
    if ((st.st_mode & S_IFMT) == S_IFLNK) {
        throw TransferFailure(
            TransferRpcCode::DestinationUnsupported,
            "destination path contains unsupported symlink"
        );
    }
    return PathInfo{true, (st.st_mode & S_IFMT) == S_IFDIR};
#endif
}
