#pragma once

#ifndef _WIN32

#include <cerrno>
#include <climits>
#include <cstdint>

#include <netdb.h>
#include <poll.h>

#include "platform/deadline.h"
#include "platform/platform.h"

namespace posix_eintr {

template <typename Result, typename Callable> Result retry(Callable call) {
    Result result;
    do {
        result = call();
    } while (result == static_cast<Result>(-1) && errno == EINTR);
    return result;
}

template <typename Pointer, typename Callable> Pointer retry_null(Callable call) {
    Pointer result;
    do {
        result = call();
    } while (result == nullptr && errno == EINTR);
    return result;
}

template <typename Callable> int retry_eai_system(Callable call) {
    int result;
    do {
        errno = 0;
        result = call();
    } while (result == EAI_SYSTEM && errno == EINTR);
    return result;
}

inline void clear_revents(struct pollfd* fds, nfds_t nfds) {
    for (nfds_t i = 0; i < nfds; ++i) {
        fds[i].revents = 0;
    }
}

inline int poll_forever(struct pollfd* fds, nfds_t nfds) {
    for (;;) {
        clear_revents(fds, nfds);
        const int result = poll(fds, nfds, -1);
        if (result >= 0 || errno != EINTR) {
            return result;
        }
    }
}

template <typename StopRequested>
int poll_forever_until(struct pollfd* fds, nfds_t nfds, StopRequested stop_requested) {
    for (;;) {
        if (stop_requested()) {
            return 0;
        }
        clear_revents(fds, nfds);
        const int result = poll(fds, nfds, -1);
        if (result >= 0 || errno != EINTR) {
            return result;
        }
    }
}

inline int poll_for_ms(struct pollfd* fds, nfds_t nfds, unsigned long timeout_ms) {
    platform::MonotonicDeadline deadline(timeout_ms);
    for (;;) {
        clear_revents(fds, nfds);
        const int timeout = deadline.remaining_ms_int();
        const int result = poll(fds, nfds, timeout);
        if (result >= 0 || errno != EINTR) {
            return result;
        }
        if (deadline.expired()) {
            return 0;
        }
    }
}

} // namespace posix_eintr

#endif
