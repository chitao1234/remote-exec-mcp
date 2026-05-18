#ifndef _WIN32

#include <cerrno>
#include <cstring>
#include <stdexcept>
#include <string>
#include <unistd.h>

#include "posix_eintr.h"
#include "posix_fd.h"
#include "wakeup_pipe.h"

WakeupPipe::WakeupPipe() : read_end_(INVALID_SOCKET), write_end_(INVALID_SOCKET), signaled_(false) {
    int fds[2];
    if (posix_eintr::retry<int>([&]() { return pipe(fds); }) != 0) {
        throw std::runtime_error(std::string("wakeup pipe creation failed: ") + std::strerror(errno));
    }
    if (!posix_fd::set_cloexec_nonblocking(fds[0]) || !posix_fd::set_cloexec_nonblocking(fds[1])) {
        close(fds[0]);
        close(fds[1]);
        throw std::runtime_error(std::string("wakeup pipe fcntl failed: ") + std::strerror(errno));
    }
    read_end_ = fds[0];
    write_end_ = fds[1];
}

WakeupPipe::~WakeupPipe() {
    if (read_end_ != INVALID_SOCKET) {
        close(read_end_);
    }
    if (!signaled_ && write_end_ != INVALID_SOCKET) {
        close(write_end_);
    }
}

void WakeupPipe::signal() {
    if (signaled_) {
        return;
    }
    signaled_ = true;
    if (write_end_ != INVALID_SOCKET) {
        close(write_end_);
        write_end_ = INVALID_SOCKET;
    }
}

SOCKET WakeupPipe::read_fd() const {
    return read_end_;
}

#endif
