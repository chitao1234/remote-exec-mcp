#pragma once

#ifndef _WIN32

#include <fcntl.h>

#include "posix_eintr.h"

namespace posix_fd {

inline bool set_cloexec(int fd) {
    const int flags = posix_eintr::retry<int>([&]() { return fcntl(fd, F_GETFD, 0); });
    return flags >= 0 && posix_eintr::retry<int>([&]() { return fcntl(fd, F_SETFD, flags | FD_CLOEXEC); }) == 0;
}

inline bool set_nonblocking(int fd, bool enabled = true) {
    const int flags = posix_eintr::retry<int>([&]() { return fcntl(fd, F_GETFL, 0); });
    if (flags < 0) {
        return false;
    }
    const int updated = enabled ? (flags | O_NONBLOCK) : (flags & ~O_NONBLOCK);
    return posix_eintr::retry<int>([&]() { return fcntl(fd, F_SETFL, updated); }) == 0;
}

inline bool set_cloexec_nonblocking(int fd) {
    return set_cloexec(fd) && set_nonblocking(fd);
}

} // namespace posix_fd

#endif
