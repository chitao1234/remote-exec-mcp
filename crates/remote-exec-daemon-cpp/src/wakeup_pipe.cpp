#ifndef _WIN32

#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <stdexcept>
#include <string>
#include <unistd.h>

#include "wakeup_pipe.h"

WakeupPipe::WakeupPipe() : read_end_(INVALID_SOCKET), write_end_(INVALID_SOCKET), signaled_(false) {
    int fds[2];
    if (pipe(fds) != 0) {
        throw std::runtime_error(std::string("wakeup pipe creation failed: ") + std::strerror(errno));
    }
    if (fcntl(fds[0], F_SETFD, fcntl(fds[0], F_GETFD) | FD_CLOEXEC) != 0 ||
        fcntl(fds[0], F_SETFL, fcntl(fds[0], F_GETFL) | O_NONBLOCK) != 0 ||
        fcntl(fds[1], F_SETFD, fcntl(fds[1], F_GETFD) | FD_CLOEXEC) != 0 ||
        fcntl(fds[1], F_SETFL, fcntl(fds[1], F_GETFL) | O_NONBLOCK) != 0) {
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
