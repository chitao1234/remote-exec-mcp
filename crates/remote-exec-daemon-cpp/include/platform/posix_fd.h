#pragma once

#ifndef _WIN32

#include <fcntl.h>
#include <unistd.h>

#include "platform/posix_eintr.h"

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

// close() consumes descriptor ownership even when it reports EINTR; do not retry it.
inline int close_consuming(int fd) {
    return ::close(fd);
}

inline void close_ignoring_errors(int fd) {
    (void)close_consuming(fd);
}

class UniqueFd {
public:
    UniqueFd() : fd_(-1) {}
    explicit UniqueFd(int fd) : fd_(fd) {}
    ~UniqueFd() { reset(); }

    UniqueFd(UniqueFd&& other) : fd_(other.release()) {}
    UniqueFd& operator=(UniqueFd&& other) {
        if (this != &other) {
            reset(other.release());
        }
        return *this;
    }

    UniqueFd(const UniqueFd&) = delete;
    UniqueFd& operator=(const UniqueFd&) = delete;

    int get() const { return fd_; }

    bool valid() const { return fd_ >= 0; }

    int release() {
        const int released = fd_;
        fd_ = -1;
        return released;
    }

    void reset(int fd = -1) {
        if (valid()) {
            close_ignoring_errors(fd_);
        }
        fd_ = fd;
    }

private:
    int fd_;
};

} // namespace posix_fd

#endif
