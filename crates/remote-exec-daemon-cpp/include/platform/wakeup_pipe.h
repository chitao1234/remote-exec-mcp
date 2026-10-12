#pragma once

#include "platform/basic_mutex.h"
#include "platform/socket.h"

struct WakeupPipe {
    WakeupPipe();
    ~WakeupPipe();
    WakeupPipe(const WakeupPipe&) = delete;
    WakeupPipe& operator=(const WakeupPipe&) = delete;

    void signal();
    SOCKET read_fd() const;

private:
    BasicMutex signal_mutex_;
    SOCKET read_end_;
    SOCKET write_end_;
    bool signaled_;
};
