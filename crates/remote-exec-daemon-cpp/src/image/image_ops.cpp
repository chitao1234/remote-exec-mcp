#include "image/image_ops.h"

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <string>

#include "core/stdio_retry.h"
#include "platform/path_utils.h"
#include "platform/platform.h"
#include "platform/scoped_file.h"
#include "rpc/rpc_failures.h"

namespace {

ImageFailure missing_image_failure(const std::string& path) {
    return ImageFailure(ImageRpcCode::Missing, "unable to locate image at `" + path + "`: No such file or directory");
}

ImageFailure not_file_image_failure(const std::string& path) {
    return ImageFailure(ImageRpcCode::NotFile, "image path `" + path + "` is not a file");
}

ImageFailure decode_failed_image(const std::string& message) {
    return ImageFailure(ImageRpcCode::DecodeFailed, message);
}

ImageFailure internal_image_failure(const std::string& message) {
    return ImageFailure(ImageRpcCode::Internal, message);
}

ImageFailure too_large_image_failure(const std::string& path) {
    return ImageFailure(ImageRpcCode::Internal, "image at `" + path + "` exceeds maximum supported size");
}

const std::size_t MAX_IMAGE_FILE_SIZE = 50U * 1024U * 1024U;

std::string read_binary_file_bytes(const std::string& path) {
    errno = 0;
    ScopedFile input(path_utils::open_file(path, "rb"));
    if (!input.valid()) {
        const int error_code = errno;
        if (error_code != 0) {
            throw internal_image_failure("unable to read image at `" + path +
                                         "`: " + errno_error::message_from_errno(error_code));
        }
        throw internal_image_failure("unable to read image at `" + path + "`");
    }
    std::string bytes;
    char buffer[8192];
    while (true) {
        const std::size_t received = stdio_retry::fread_some(input.get(), buffer, sizeof(buffer));
        if (received > 0U) {
            bytes.append(buffer, received);
        }
        if (received < sizeof(buffer)) {
            if (std::ferror(input.get()) != 0) {
                throw internal_image_failure("unable to read image at `" + path + "`");
            }
            break;
        }
    }
    return bytes;
}

void require_regular_image_file(const std::string& path) {
    path_utils::PathMetadata metadata;
    if (path_utils::path_metadata(path, &metadata)) {
        if (!metadata.is_regular_file) {
            throw not_file_image_failure(path);
        }
        if (metadata.size > static_cast<std::uint64_t>(MAX_IMAGE_FILE_SIZE)) {
            throw too_large_image_failure(path);
        }
        return;
    }

    const int error_code = errno;
    if (error_code == ENOENT || error_code == ENOTDIR) {
        throw missing_image_failure(path);
    }

    throw internal_image_failure("unable to access image at `" + path +
                                 "`: " + errno_error::message_from_errno(error_code));
}

std::string image_mime_type(const std::string& path, const std::string& bytes) {
    if (bytes.size() >= 8 && std::memcmp(bytes.data(), "\x89PNG\r\n\x1A\n", 8) == 0) {
        return "image/png";
    }
    if (bytes.size() >= 3 && static_cast<unsigned char>(bytes[0]) == 0xFF &&
        static_cast<unsigned char>(bytes[1]) == 0xD8 && static_cast<unsigned char>(bytes[2]) == 0xFF) {
        return "image/jpeg";
    }
    if (bytes.size() >= 12 && std::memcmp(bytes.data(), "RIFF", 4) == 0 &&
        std::memcmp(bytes.data() + 8, "WEBP", 4) == 0) {
        return "image/webp";
    }
    throw decode_failed_image("unable to process image at `" + path + "`: unsupported image format");
}

} // namespace

ImageReadResult read_image_original(const std::string& path) {
    require_regular_image_file(path);
    const std::string bytes = read_binary_file_bytes(path);
    ImageReadResult result;
    result.mime_type = image_mime_type(path, bytes);
    result.bytes = bytes;
    return result;
}
