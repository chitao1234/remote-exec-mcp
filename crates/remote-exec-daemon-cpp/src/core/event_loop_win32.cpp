#ifdef _WIN32

#include "core/event_loop.h"

#include <algorithm>
#include <cstring>
#include <deque>
#include <map>
#include <set>
#include <utility>
#include <vector>

#include "core/logging.h"
#include "platform/basic_mutex.h"
#include "platform/platform.h"
#include "platform/win32_socket_compat.h"

namespace {

const int MAX_SELECT_TIMEOUT_MS = 60000;

struct ReadyCallback {
    ReadyCallback(const std::function<void(int)>& callback_value, int bits_value)
        : callback(callback_value), bits(bits_value) {}

    std::function<void(int)> callback;
    int bits;
};

bool set_socket_nonblocking_option(SOCKET socket, bool enabled) {
    u_long mode = enabled ? 1UL : 0UL;
    return ioctlsocket(socket, FIONBIO, &mode) == 0;
}

} // namespace

struct EventLoop::Impl {
    struct Entry {
        Entry() : interest(0) {}
        int interest;
        std::function<void(int)> on_events;
    };

    Impl()
        : next_timer_id(1), wake_read(INVALID_SOCKET), wake_write(INVALID_SOCKET),
          stop_requested(false) {}

    std::map<SOCKET, Entry> fds;
    std::deque<std::function<void()>> posted;
    std::map<std::pair<std::uint64_t, TimerId>, std::function<void()>> timers;
    std::set<TimerId> cancelled_timers;
    TimerId next_timer_id;
    BasicMutex mutex;
    SOCKET wake_read;
    SOCKET wake_write;
    bool stop_requested;
};

EventLoop::EventLoop() : impl_(new Impl()) {
    SOCKET listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (listener == INVALID_SOCKET) {
        return;
    }

    sockaddr_in address;
    std::memset(&address, 0, sizeof(address));
    address.sin_family = AF_INET;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    address.sin_port = 0;

    if (bind(listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0
        || listen(listener, 1) != 0) {
        closesocket(listener);
        return;
    }

    int address_len = sizeof(address);
    if (getsockname(listener, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        closesocket(listener);
        return;
    }

    SOCKET connector = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (connector == INVALID_SOCKET) {
        closesocket(listener);
        return;
    }
    if (connect(connector, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
        closesocket(connector);
        closesocket(listener);
        return;
    }

    SOCKET accepted = accept(listener, nullptr, nullptr);
    closesocket(listener);
    if (accepted == INVALID_SOCKET) {
        closesocket(connector);
        return;
    }

    if (!set_socket_nonblocking_option(accepted, true)
        || !set_socket_nonblocking_option(connector, true)) {
        closesocket(accepted);
        closesocket(connector);
        return;
    }

    impl_->wake_read = accepted;
    impl_->wake_write = connector;
}

EventLoop::~EventLoop() {
    if (impl_->wake_read != INVALID_SOCKET) {
        closesocket(impl_->wake_read);
    }
    if (impl_->wake_write != INVALID_SOCKET) {
        closesocket(impl_->wake_write);
    }
}

bool EventLoop::valid() const {
    return impl_->wake_read != INVALID_SOCKET && impl_->wake_write != INVALID_SOCKET;
}

std::size_t EventLoop::max_watched_sockets() {
    return static_cast<std::size_t>(FD_SETSIZE) - 1U;
}

bool EventLoop::add(SOCKET fd, int interest, const std::function<void(int)>& on_events) {
    if (!valid() || fd == INVALID_SOCKET) {
        return false;
    }
    std::map<SOCKET, Impl::Entry>::iterator existing = impl_->fds.find(fd);
    if (existing == impl_->fds.end()
        && impl_->fds.size() + 1U >= static_cast<std::size_t>(FD_SETSIZE)) {
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
    if (impl_->wake_write != INVALID_SOCKET) {
        const char byte = 1;
        (void)send(impl_->wake_write, &byte, 1, 0);
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
    if (impl_->wake_write != INVALID_SOCKET) {
        const char byte = 1;
        (void)send(impl_->wake_write, &byte, 1, 0);
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

        fd_set read_fds;
        fd_set write_fds;
        FD_ZERO(&read_fds);
        FD_ZERO(&write_fds);
        FD_SET(impl_->wake_read, &read_fds);
        std::vector<SOCKET> watched;
        watched.reserve(impl_->fds.size());
        for (std::map<SOCKET, Impl::Entry>::const_iterator it = impl_->fds.begin();
             it != impl_->fds.end();
             ++it) {
            if (it->second.interest == 0) {
                continue;
            }
            if ((it->second.interest & IO_READ) != 0) {
                FD_SET(it->first, &read_fds);
            }
            if ((it->second.interest & IO_WRITE) != 0) {
                FD_SET(it->first, &write_fds);
            }
            watched.push_back(it->first);
        }

        struct timeval timeout;
        struct timeval* timeout_ptr = nullptr;
        {
            BasicLockGuard lock(impl_->mutex);
            if (!impl_->timers.empty()) {
                const std::uint64_t now = platform::monotonic_ms();
                const std::uint64_t deadline = impl_->timers.begin()->first.first;
                const std::uint64_t remaining = deadline <= now ? 0U : deadline - now;
                const std::uint64_t bounded =
                    remaining > static_cast<std::uint64_t>(MAX_SELECT_TIMEOUT_MS)
                        ? static_cast<std::uint64_t>(MAX_SELECT_TIMEOUT_MS)
                        : remaining;
                timeout.tv_sec = static_cast<long>(bounded / 1000U);
                timeout.tv_usec = static_cast<long>((bounded % 1000U) * 1000U);
                timeout_ptr = &timeout;
            }
        }

        const int ready = select(0, &read_fds, &write_fds, nullptr, timeout_ptr);
        if (ready == SOCKET_ERROR) {
            const int error = WSAGetLastError();
            if (error == WSAEINTR) {
                continue;
            }
            log_message(
                LOG_ERROR,
                "event_loop",
                std::string("select failed with WSA error ") + std::to_string(error)
            );
            return;
        }
        if (ready == 0) {
            continue;
        }

        if (FD_ISSET(impl_->wake_read, &read_fds)) {
            char buffer[256];
            while (recv(impl_->wake_read, buffer, sizeof(buffer), 0) > 0) {
            }
        }

        std::vector<ReadyCallback> callbacks;
        callbacks.reserve(watched.size());
        for (std::size_t i = 0; i < watched.size(); ++i) {
            int bits = 0;
            if (FD_ISSET(watched[i], &read_fds)) {
                bits |= IO_READ;
            }
            if (FD_ISSET(watched[i], &write_fds)) {
                bits |= IO_WRITE;
            }
            if (bits == 0) {
                continue;
            }
            std::map<SOCKET, Impl::Entry>::const_iterator it = impl_->fds.find(watched[i]);
            if (it == impl_->fds.end()) {
                continue;
            }
            callbacks.push_back(ReadyCallback(it->second.on_events, bits));
        }
        for (std::size_t i = 0; i < callbacks.size(); ++i) {
            callbacks[i].callback(callbacks[i].bits);
        }
    }
}

#endif
