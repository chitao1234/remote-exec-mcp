#pragma once

#ifndef _WIN32

#include <cerrno>
#include <cstddef>
#include <cstring>
#include <fcntl.h>
#include <poll.h>
#include <stdlib.h>
#include <string>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <termios.h>
#include <unistd.h>

#include "platform/basic_mutex.h"
#include "platform/posix_eintr.h"
#include "remote_exec_cpp_config.h"

#if REMOTE_EXEC_CPP_HAVE_POSIX_OPENPT
extern "C" int posix_openpt(int flags);
#endif
#if REMOTE_EXEC_CPP_HAVE_GRANTPT
extern "C" int grantpt(int fd);
#endif
#if REMOTE_EXEC_CPP_HAVE_UNLOCKPT
extern "C" int unlockpt(int fd);
#endif
#if REMOTE_EXEC_CPP_HAVE_PTSNAME
extern "C" char* ptsname(int fd);
#endif

namespace posix_fd {

inline bool set_cloexec(int fd) {
    const int flags = posix_eintr::retry<int>([&]() { return fcntl(fd, F_GETFD, 0); });
    return flags >= 0 && posix_eintr::retry<int>([&]() {
                             return fcntl(fd, F_SETFD, flags | FD_CLOEXEC);
                         }) == 0;
}

inline bool set_nonblocking(int fd, bool enabled = true) {
    const int flags = posix_eintr::retry<int>([&]() { return fcntl(fd, F_GETFL, 0); });
    if (flags < 0) {
        return false;
    }
    const int updated = enabled ? (flags | O_NONBLOCK) : (flags & ~O_NONBLOCK);
    return posix_eintr::retry<int>([&]() { return fcntl(fd, F_SETFL, updated); }) == 0;
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
#if REMOTE_EXEC_CPP_HAVE_PIPE2 && REMOTE_EXEC_CPP_HAVE_O_CLOEXEC
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
#if REMOTE_EXEC_CPP_HAVE_O_CLOEXEC
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
    ssize_t result;
    do {
        result = ::read(fd, data, size);
    } while (result == static_cast<ssize_t>(-1) && errno == EINTR);
    return result;
}

inline ssize_t write_retry(int fd, const void* data, std::size_t size) {
    ssize_t result;
    do {
        result = ::write(fd, data, size);
    } while (result == static_cast<ssize_t>(-1) && errno == EINTR);
    return result;
}

inline int dup_to(int source_fd, int target_fd) {
    return posix_eintr::retry<int>([&]() { return dup2(source_fd, target_fd); });
}

inline int ioctl_retry_no_arg(int fd, unsigned long request) {
#ifdef __HAIKU__
    return posix_eintr::retry<int>([&]() { return ioctl(fd, request, static_cast<void*>(nullptr)); }
    );
#else
    return posix_eintr::retry<int>([&]() { return ioctl(fd, request, 0); });
#endif
}

template <typename Argument> int ioctl_retry(int fd, unsigned long request, Argument* argument) {
    return posix_eintr::retry<int>([&]() { return ioctl(fd, request, argument); });
}

inline int access_path(const char* path, int mode) {
    return posix_eintr::retry<int>([&]() { return access(path, mode); });
}

inline int change_directory(const char* path) {
    return posix_eintr::retry<int>([&]() { return chdir(path); });
}

inline bool pty_api_available() {
    return REMOTE_EXEC_CPP_HAVE_POSIX_OPENPT && REMOTE_EXEC_CPP_HAVE_GRANTPT
           && REMOTE_EXEC_CPP_HAVE_UNLOCKPT
           && (REMOTE_EXEC_CPP_HAVE_PTSNAME_R || REMOTE_EXEC_CPP_HAVE_PTSNAME);
}

inline int open_pty_master(int flags) {
#if REMOTE_EXEC_CPP_HAVE_POSIX_OPENPT
    return posix_eintr::retry<int>([&]() { return posix_openpt(flags); });
#else
    (void)flags;
    errno = ENOSYS;
    return -1;
#endif
}

inline int grant_pty(int fd) {
#if REMOTE_EXEC_CPP_HAVE_GRANTPT
    return posix_eintr::retry<int>([&]() { return grantpt(fd); });
#else
    (void)fd;
    errno = ENOSYS;
    return -1;
#endif
}

inline int unlock_pty(int fd) {
#if REMOTE_EXEC_CPP_HAVE_UNLOCKPT
    return posix_eintr::retry<int>([&]() { return unlockpt(fd); });
#else
    (void)fd;
    errno = ENOSYS;
    return -1;
#endif
}

inline bool pty_slave_path(int master_fd, std::string* slave_path) {
    if (slave_path == nullptr) {
        errno = EINVAL;
        return false;
    }
#if REMOTE_EXEC_CPP_HAVE_PTSNAME_R
    char pts_buf[256];
    const int result = ptsname_r(master_fd, pts_buf, sizeof(pts_buf));
    if (result != 0) {
        errno = result;
        return false;
    }
    *slave_path = pts_buf;
    return true;
#endif

#if REMOTE_EXEC_CPP_HAVE_PTSNAME
    static BasicMutex ptsname_mutex;
    BasicLockGuard ptsname_lock(ptsname_mutex);
    char* slave_name = ptsname(master_fd);
    if (slave_name == nullptr) {
        return false;
    }
    *slave_path = slave_name;
    return true;
#else
    (void)master_fd;
    errno = ENOSYS;
    return false;
#endif
}

inline void make_controlling_terminal_best_effort(int fd) {
#ifdef TIOCSCTTY
    (void)ioctl_retry_no_arg(fd, TIOCSCTTY);
#else
    (void)fd;
#endif
}

inline bool set_pty_window_size(int fd, unsigned short rows, unsigned short cols) {
    struct winsize size;
    std::memset(&size, 0, sizeof(size));
    size.ws_row = rows;
    size.ws_col = cols;
    return ioctl_retry(fd, TIOCSWINSZ, &size) == 0;
}

inline int poll_readable_or_hangup(int fd, unsigned long timeout_ms, bool* readable) {
    if (readable == nullptr) {
        errno = EINVAL;
        return -1;
    }
    struct pollfd descriptor;
    descriptor.fd = fd;
    descriptor.events = POLLIN | POLLHUP | POLLERR;
    descriptor.revents = 0;

    const int result = posix_eintr::poll_for_ms(&descriptor, 1, timeout_ms);
    if (result < 0) {
        return -1;
    }
    *readable = result > 0 && (descriptor.revents & (POLLIN | POLLHUP | POLLERR)) != 0;
    return 0;
}

inline int wait_until_readable_or_hangup(int fd) {
    struct pollfd descriptor;
    descriptor.fd = fd;
    descriptor.events = POLLIN | POLLHUP | POLLERR;
    descriptor.revents = 0;

    for (;;) {
        const int result = posix_eintr::poll_forever(&descriptor, 1);
        if (result > 0) {
            return 0;
        }
        if (result < 0) {
            return -1;
        }
    }
}

inline int wait_until_writable_or_hangup(int fd) {
    struct pollfd descriptor;
    descriptor.fd = fd;
    descriptor.events = POLLOUT | POLLHUP | POLLERR;
    descriptor.revents = 0;

    for (;;) {
        const int result = posix_eintr::poll_forever(&descriptor, 1);
        if (result > 0) {
            return 0;
        }
        if (result < 0) {
            return -1;
        }
    }
}

inline void write_signal_safe_wakeup_byte(int fd) {
    const unsigned char wakeup_byte = 1U;
    // Async-signal-safe notification is best effort; do not loop in a handler.
    const ssize_t ignored = ::write(fd, &wakeup_byte, 1U);
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
