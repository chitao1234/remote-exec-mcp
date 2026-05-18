#ifndef _WIN32

#include <fcntl.h>
#include <unistd.h>

#include "wakeup_pipe.h"

WakeupPipe::WakeupPipe() : read_end_(INVALID_SOCKET), write_end_(INVALID_SOCKET), signaled_(false) {
    int fds[2];
    if (pipe(fds) != 0) {
        return;
    }
    fcntl(fds[0], F_SETFD, fcntl(fds[0], F_GETFD) | FD_CLOEXEC);
    fcntl(fds[0], F_SETFL, fcntl(fds[0], F_GETFL) | O_NONBLOCK);
    fcntl(fds[1], F_SETFD, fcntl(fds[1], F_GETFD) | FD_CLOEXEC);
    fcntl(fds[1], F_SETFL, fcntl(fds[1], F_GETFL) | O_NONBLOCK);
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
