#ifndef _WIN32

#include "core/event_loop.h"

#include <algorithm>
#include <cerrno>
#include <deque>
#include <map>
#include <set>
#include <utility>
#include <vector>

#include <poll.h>

#include "core/logging.h"
#include "platform/basic_mutex.h"
#include "platform/platform.h"
#include "platform/posix_eintr.h"
#include "platform/posix_fd.h"

namespace {

const int MAX_POLL_TIMEOUT_MS = 60000;
const std::size_t MAX_WATCHED_SOCKETS = 4096U;

struct ReadyCallback {
    ReadyCallback(const std::function<void(int)>& callback_value, int bits_value)
        : callback(callback_value), bits(bits_value) {}

    std::function<void(int)> callback;
    int bits;
};

} // namespace

struct EventLoop::Impl {
    struct Entry {
        Entry() : interest(0) {}
        int interest;
        std::function<void(int)> on_events;
    };

    Impl()
        : next_timer_id(1), wake_read(-1), wake_write(-1), stop_requested(false),
          capacity(MAX_WATCHED_SOCKETS) {}

    std::map<SOCKET, Entry> fds;
    std::deque<std::function<void()>> posted;
    std::map<std::pair<std::uint64_t, TimerId>, std::function<void()>> timers;
    std::set<TimerId> cancelled_timers;
    TimerId next_timer_id;
    BasicMutex mutex;
    int wake_read;
    int wake_write;
    bool stop_requested;
    std::size_t capacity;
};

EventLoop::EventLoop(std::size_t capacity) : impl_(new Impl()) {
    if (capacity != 0U) {
        impl_->capacity = capacity;
    }
    int fds[2];
    if (posix_fd::create_cloexec_pipe(fds) != 0) {
        return;
    }
    if (!posix_fd::set_nonblocking(fds[0]) || !posix_fd::set_nonblocking(fds[1])) {
        posix_fd::close_ignoring_errors(fds[0]);
        posix_fd::close_ignoring_errors(fds[1]);
        return;
    }
    impl_->wake_read = fds[0];
    impl_->wake_write = fds[1];
}

EventLoop::~EventLoop() {
    if (impl_->wake_read >= 0) {
        posix_fd::close_ignoring_errors(impl_->wake_read);
    }
    if (impl_->wake_write >= 0) {
        posix_fd::close_ignoring_errors(impl_->wake_write);
    }
}

bool EventLoop::valid() const {
    return impl_->wake_read >= 0 && impl_->wake_write >= 0;
}

std::size_t EventLoop::max_watched_sockets() {
    return MAX_WATCHED_SOCKETS;
}

bool EventLoop::add(SOCKET fd, int interest, const std::function<void(int)>& on_events) {
    if (!valid() || fd < 0) {
        return false;
    }
    if (impl_->fds.find(fd) == impl_->fds.end() && impl_->fds.size() >= impl_->capacity) {
        return false;
    }
    Impl::Entry& entry = impl_->fds[fd];
    entry.interest = interest;
    entry.on_events = on_events;
    return true;
}

bool EventLoop::set_interest(SOCKET fd, int interest) {
    std::map<SOCKET, Impl::Entry>::iterator it = impl_->fds.find(fd);
    if (it == impl_->fds.end()) {
        return false;
    }
    it->second.interest = interest;
    return true;
}

void EventLoop::remove(SOCKET fd) {
    impl_->fds.erase(fd);
}

void EventLoop::post(const std::function<void()>& fn) {
    {
        BasicLockGuard lock(impl_->mutex);
        impl_->posted.push_back(fn);
    }
    if (impl_->wake_write >= 0) {
        const char byte = 1;
        (void)posix_fd::write_retry(impl_->wake_write, &byte, 1);
    }
}

EventLoop::TimerId EventLoop::add_timer(
    std::uint64_t deadline_ms,
    const std::function<void()>& fn
) {
    BasicLockGuard lock(impl_->mutex);
    const TimerId id = impl_->next_timer_id++;
    impl_->timers[std::make_pair(deadline_ms, id)] = fn;
    return id;
}

void EventLoop::cancel_timer(TimerId id) {
    BasicLockGuard lock(impl_->mutex);
    impl_->cancelled_timers.insert(id);
    for (std::map<std::pair<std::uint64_t, TimerId>, std::function<void()>>::iterator it =
             impl_->timers.begin();
         it != impl_->timers.end();
         ++it) {
        if (it->first.second == id) {
            impl_->timers.erase(it);
            return;
        }
    }
}

void EventLoop::stop() {
    {
        BasicLockGuard lock(impl_->mutex);
        impl_->stop_requested = true;
    }
    if (impl_->wake_write >= 0) {
        const char byte = 1;
        (void)posix_fd::write_retry(impl_->wake_write, &byte, 1);
    }
}

void EventLoop::run() {
    std::vector<std::function<void()>> due_timers;

    for (;;) {
        {
            BasicLockGuard lock(impl_->mutex);
            if (impl_->stop_requested) {
                return;
            }
        }

        due_timers.clear();
        {
            BasicLockGuard lock(impl_->mutex);
            const std::uint64_t now = platform::monotonic_ms();
            std::map<std::pair<std::uint64_t, TimerId>, std::function<void()>>::iterator it =
                impl_->timers.begin();
            while (it != impl_->timers.end() && it->first.first <= now) {
                const TimerId id = it->first.second;
                if (impl_->cancelled_timers.erase(id) == 0U) {
                    due_timers.push_back(it->second);
                }
                impl_->timers.erase(it++);
            }
        }
        for (std::size_t i = 0; i < due_timers.size(); ++i) {
            due_timers[i]();
        }

        std::deque<std::function<void()>> posted;
        {
            BasicLockGuard lock(impl_->mutex);
            posted.swap(impl_->posted);
        }
        for (std::deque<std::function<void()>>::iterator it = posted.begin(); it != posted.end();
             ++it) {
            (*it)();
        }

        int timeout_ms = -1;
        {
            BasicLockGuard lock(impl_->mutex);
            if (!impl_->timers.empty()) {
                const std::uint64_t now = platform::monotonic_ms();
                const std::uint64_t deadline = impl_->timers.begin()->first.first;
                if (deadline <= now) {
                    timeout_ms = 0;
                } else {
                    const std::uint64_t remaining = deadline - now;
                    timeout_ms = remaining > static_cast<std::uint64_t>(MAX_POLL_TIMEOUT_MS)
                                     ? MAX_POLL_TIMEOUT_MS
                                     : static_cast<int>(remaining);
                }
            }
        }

        std::vector<struct pollfd> poll_fds;
        std::vector<SOCKET> poll_sockets;
        poll_fds.reserve(impl_->fds.size() + 1U);
        poll_sockets.reserve(impl_->fds.size());
        for (std::map<SOCKET, Impl::Entry>::const_iterator it = impl_->fds.begin();
             it != impl_->fds.end();
             ++it) {
            if (it->second.interest == 0) {
                continue;
            }
            struct pollfd descriptor;
            descriptor.fd = it->first;
            descriptor.events = 0;
            if ((it->second.interest & IO_READ) != 0) {
                descriptor.events |= POLLIN;
            }
            if ((it->second.interest & IO_WRITE) != 0) {
                descriptor.events |= POLLOUT;
            }
            descriptor.revents = 0;
            poll_fds.push_back(descriptor);
            poll_sockets.push_back(it->first);
        }

        struct pollfd wakeup;
        wakeup.fd = impl_->wake_read;
        wakeup.events = POLLIN;
        wakeup.revents = 0;
        poll_fds.push_back(wakeup);

        const int ready = poll(poll_fds.data(), poll_fds.size(), timeout_ms);
        if (ready < 0) {
            if (errno == EINTR) {
                continue;
            }
            log_message(
                LOG_ERROR,
                "event_loop",
                std::string("poll failed: ") + errno_error::message_from_errno(errno)
            );
            return;
        }
        if (ready == 0) {
            continue;
        }

        struct pollfd& wakeup_result = poll_fds.back();
        if (wakeup_result.revents != 0) {
            char buffer[256];
            while (posix_fd::read_retry(impl_->wake_read, buffer, sizeof(buffer)) > 0) {
            }
        }

        std::vector<ReadyCallback> callbacks;
        callbacks.reserve(poll_sockets.size());
        for (std::size_t i = 0; i < poll_sockets.size(); ++i) {
            const short revents = poll_fds[i].revents;
            if (revents == 0) {
                continue;
            }
            std::map<SOCKET, Impl::Entry>::const_iterator it = impl_->fds.find(poll_sockets[i]);
            if (it == impl_->fds.end()) {
                continue;
            }
            int bits = 0;
            if ((revents & (POLLIN | POLLHUP)) != 0) {
                bits |= IO_READ;
            }
            if ((revents & POLLOUT) != 0) {
                bits |= IO_WRITE;
            }
            if ((revents & (POLLERR | POLLNVAL)) != 0) {
                bits |= IO_READ | IO_WRITE;
            }
            if (bits != 0) {
                callbacks.push_back(ReadyCallback(it->second.on_events, bits));
            }
        }
        for (std::size_t i = 0; i < callbacks.size(); ++i) {
            callbacks[i].callback(callbacks[i].bits);
        }
    }
}

#endif
