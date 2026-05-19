#pragma once

#include <cerrno>
#include <cstddef>
#include <cstdio>

namespace stdio_retry {

inline std::size_t fread_some(FILE* file, char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        errno = 0;
        const std::size_t received = std::fread(data + offset, 1, size - offset, file);
        offset += received;
        if (offset == size) {
            return offset;
        }
        if (std::ferror(file) != 0) {
#ifndef _WIN32
            if (errno == EINTR) {
                std::clearerr(file);
                continue;
            }
#endif
            return offset;
        }
        return offset;
    }
    return offset;
}

inline bool fwrite_all(FILE* file, const char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        errno = 0;
        const std::size_t written = std::fwrite(data + offset, 1, size - offset, file);
        offset += written;
        if (offset == size) {
            return true;
        }
        if (std::ferror(file) != 0) {
#ifndef _WIN32
            if (errno == EINTR) {
                std::clearerr(file);
                continue;
            }
#endif
            return false;
        }
        if (written == 0U) {
            return false;
        }
    }
    return true;
}

} // namespace stdio_retry
