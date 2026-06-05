#ifndef _WIN32

#include <cerrno>
#include <stdexcept>
#include <string>

#include "platform/platform.h"
#include "platform/posix_fd.h"
#include "platform/wakeup_pipe.h"

WakeupPipe::WakeupPipe() : read_end_(INVALID_SOCKET), write_end_(INVALID_SOCKET), signaled_(false) {
    int fds[2];
    if (posix_fd::create_cloexec_pipe(fds) != 0) {
        throw std::runtime_error(errno_error::operation_failed("wakeup pipe creation", errno));
    }
    if (!posix_fd::set_nonblocking(fds[0]) || !posix_fd::set_nonblocking(fds[1])) {
        posix_fd::close_ignoring_errors(fds[0]);
        posix_fd::close_ignoring_errors(fds[1]);
        throw std::runtime_error(errno_error::operation_failed("wakeup pipe fcntl", errno));
    }
    read_end_ = fds[0];
    write_end_ = fds[1];
}

WakeupPipe::~WakeupPipe() {
    if (read_end_ != INVALID_SOCKET) {
        posix_fd::close_ignoring_errors(read_end_);
    }
    if (!signaled_ && write_end_ != INVALID_SOCKET) {
        posix_fd::close_ignoring_errors(write_end_);
    }
}

void WakeupPipe::signal() {
    if (signaled_) {
        return;
    }
    signaled_ = true;
    if (write_end_ != INVALID_SOCKET) {
        posix_fd::close_ignoring_errors(write_end_);
        write_end_ = INVALID_SOCKET;
    }
}

SOCKET WakeupPipe::read_fd() const {
    return read_end_;
}

#endif
