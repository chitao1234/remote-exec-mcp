#pragma once

#ifndef _WIN32

#include <cerrno>
#include <cstddef>
#include <fcntl.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/types.h>
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

inline int create_pipe(int fds[2]) {
    return posix_eintr::retry<int>([&]() { return pipe(fds); });
}

inline int create_cloexec_pipe(int fds[2]) {
#if defined(__linux__) && defined(O_CLOEXEC)
    if (posix_eintr::retry<int>([&]() { return pipe2(fds, O_CLOEXEC); }) == 0) {
        return 0;
    }
    if (errno != EINVAL && errno != ENOSYS) {
        return -1;
    }
#endif

    if (create_pipe(fds) != 0) {
        return -1;
    }
    if (set_cloexec(fds[0]) && set_cloexec(fds[1])) {
        return 0;
    }
    const int saved_errno = errno;
    close_ignoring_errors(fds[0]);
    close_ignoring_errors(fds[1]);
    errno = saved_errno;
    return -1;
}

inline int open_path(const char* path, int flags) {
    return posix_eintr::retry<int>([&]() { return open(path, flags); });
}

inline int open_path(const char* path, int flags, mode_t mode) {
    return posix_eintr::retry<int>([&]() { return open(path, flags, mode); });
}

inline int open_cloexec_path(const char* path, int flags) {
    int raw_fd = -1;
#ifdef O_CLOEXEC
    raw_fd = open_path(path, flags | O_CLOEXEC);
    if (raw_fd >= 0) {
        return raw_fd;
    }
    if (errno != EINVAL) {
        return -1;
    }
#endif
    raw_fd = open_path(path, flags);
    if (raw_fd < 0) {
        return -1;
    }
    if (set_cloexec(raw_fd)) {
        return raw_fd;
    }
    const int saved_errno = errno;
    close_ignoring_errors(raw_fd);
    errno = saved_errno;
    return -1;
}

inline ssize_t read_retry(int fd, void* data, std::size_t size) {
    return posix_eintr::retry<ssize_t>([&]() { return read(fd, data, size); });
}

inline ssize_t write_retry(int fd, const void* data, std::size_t size) {
    return posix_eintr::retry<ssize_t>([&]() { return write(fd, data, size); });
}

inline int dup_to(int source_fd, int target_fd) {
    return posix_eintr::retry<int>([&]() { return dup2(source_fd, target_fd); });
}

template <typename Argument>
int ioctl_retry(int fd, unsigned long request, Argument argument) {
    return posix_eintr::retry<int>([&]() { return ioctl(fd, request, argument); });
}

inline int access_path(const char* path, int mode) {
    return posix_eintr::retry<int>([&]() { return access(path, mode); });
}

inline int change_directory(const char* path) {
    return posix_eintr::retry<int>([&]() { return chdir(path); });
}

inline void write_signal_safe_wakeup_byte(int fd) {
    const unsigned char byte = 1U;
    // Async-signal-safe notification is best effort; do not loop in a handler.
    const ssize_t ignored = write(fd, &byte, 1U);
    (void)ignored;
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
