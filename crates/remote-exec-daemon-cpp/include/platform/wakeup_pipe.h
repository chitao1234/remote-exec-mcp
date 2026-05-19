#pragma once

#include "platform/socket.h"

struct WakeupPipe {
    WakeupPipe();
    ~WakeupPipe();
    WakeupPipe(const WakeupPipe&) = delete;
    WakeupPipe& operator=(const WakeupPipe&) = delete;

    void signal();
    SOCKET read_fd() const;

private:
    SOCKET read_end_;
    SOCKET write_end_;
    bool signaled_;
};
