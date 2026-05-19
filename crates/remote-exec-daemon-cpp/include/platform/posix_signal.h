#pragma once

#ifndef _WIN32

#include <cstring>
#include <signal.h>

namespace posix_signal {

typedef void (*SignalHandler)(int);

inline int install_handler(int signal_number, SignalHandler handler, int flags) {
    struct sigaction action;
    std::memset(&action, 0, sizeof(action));
    action.sa_handler = handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = flags;
    return sigaction(signal_number, &action, nullptr);
}

inline int ignore_signal(int signal_number) {
    return install_handler(signal_number, SIG_IGN, 0);
}

inline int restore_default(int signal_number) {
    return install_handler(signal_number, SIG_DFL, 0);
}

} // namespace posix_signal

#endif
