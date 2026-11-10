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

// A select() set can watch at most FD_SETSIZE sockets. Large loops split
// their sockets across several sets; the first set blocks for the common
// timeout and the rest are polled without waiting, so one loop thread can
// still own more sockets than a single fd_set allows.
const unsigned long MULTI_SELECT_WAIT_MS = 20UL;

std::size_t app_sockets_per_select_group() {
    return static_cast<std::size_t>(FD_SETSIZE) - 1U;
}

std::size_t select_group_count(std::size_t app_sockets) {
    const std::size_t per_group = app_sockets_per_select_group();
    if (per_group == 0U) {
        return 1U;
    }
    return app_sockets == 0U ? 1U : (app_sockets + per_group - 1U) / per_group;
}

struct SelectGroup {
    SelectGroup() : read_fds(), write_fds(), watched(), wake_ready(false) {
        FD_ZERO(&read_fds);
        FD_ZERO(&write_fds);
    }

    fd_set read_fds;
    fd_set write_fds;
    std::vector<SOCKET> watched;
    bool wake_ready;
};

} // namespace

struct EventLoop::Impl {
    struct Entry {
        Entry() : interest(0) {}
        int interest;
        std::function<void(int)> on_events;
    };

    Impl()
        : next_timer_id(1), wake_read(INVALID_SOCKET), wake_write(INVALID_SOCKET),
          stop_requested(false), capacity(static_cast<std::size_t>(FD_SETSIZE) - 1U) {}

    std::map<SOCKET, Entry> fds;
    std::deque<std::function<void()>> posted;
    std::map<std::pair<std::uint64_t, TimerId>, std::function<void()>> timers;
    std::set<TimerId> cancelled_timers;
    TimerId next_timer_id;
    BasicMutex mutex;
    SOCKET wake_read;
    SOCKET wake_write;
    bool stop_requested;
    std::size_t capacity;
};

EventLoop::EventLoop(std::size_t capacity) : impl_(new Impl()) {
    if (capacity != 0U) {
        impl_->capacity = capacity;
    }
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
    if (existing == impl_->fds.end() && impl_->fds.size() >= impl_->capacity) {
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

        std::vector<SOCKET> active;
        active.reserve(impl_->fds.size());
        for (std::map<SOCKET, Impl::Entry>::const_iterator it = impl_->fds.begin();
             it != impl_->fds.end();
             ++it) {
            if (it->second.interest != 0) {
                active.push_back(it->first);
            }
        }

        const std::size_t per_group = app_sockets_per_select_group();
        const std::size_t group_count = select_group_count(active.size());
        std::vector<SelectGroup> groups(group_count);
        for (std::size_t i = 0; i < groups.size(); ++i) {
            if (i == 0U) {
                // Only group 0 blocks; it carries the wake socket. The other
                // groups are polled without waiting after it returns.
                FD_SET(impl_->wake_read, &groups[0].read_fds);
            }
            groups[i].watched.reserve(
                i + 1U == groups.size() ? active.size() - i * per_group : per_group
            );
        }
        for (std::size_t i = 0; i < active.size(); ++i) {
            const std::size_t group = std::min(i / per_group, groups.size() - 1U);
            const Impl::Entry& entry = impl_->fds.find(active[i])->second;
            if ((entry.interest & IO_READ) != 0) {
                FD_SET(active[i], &groups[group].read_fds);
            }
            if ((entry.interest & IO_WRITE) != 0) {
                FD_SET(active[i], &groups[group].write_fds);
            }
            groups[group].watched.push_back(active[i]);
        }

        struct timeval timeout;
        struct timeval zero_timeout;
        zero_timeout.tv_sec = 0;
        zero_timeout.tv_usec = 0;
        std::uint64_t wait_ms = 0U;
        bool have_timeout = false;
        {
            BasicLockGuard lock(impl_->mutex);
            if (!impl_->timers.empty()) {
                const std::uint64_t now = platform::monotonic_ms();
                const std::uint64_t deadline = impl_->timers.begin()->first.first;
                wait_ms = deadline <= now ? 0U : deadline - now;
                have_timeout = true;
            }
        }
        if (group_count > 1U) {
            wait_ms = have_timeout ? std::min<std::uint64_t>(wait_ms, MULTI_SELECT_WAIT_MS)
                                   : static_cast<std::uint64_t>(MULTI_SELECT_WAIT_MS);
            have_timeout = true;
        } else if (have_timeout) {
            wait_ms =
                std::min<std::uint64_t>(wait_ms, static_cast<std::uint64_t>(MAX_SELECT_TIMEOUT_MS));
        }
        struct timeval* timeout_ptr = nullptr;
        if (have_timeout) {
            timeout.tv_sec = static_cast<long>(wait_ms / 1000U);
            timeout.tv_usec = static_cast<long>((wait_ms % 1000U) * 1000U);
            timeout_ptr = &timeout;
        }

        bool wake_ready = false;
        for (std::size_t i = 0; i < groups.size(); ++i) {
            const int ready = select(
                0,
                &groups[i].read_fds,
                &groups[i].write_fds,
                nullptr,
                i == 0U ? timeout_ptr : &zero_timeout
            );
            if (ready == SOCKET_ERROR) {
                const int error = WSAGetLastError();
                if (error == WSAEINTR) {
                    FD_ZERO(&groups[i].read_fds);
                    FD_ZERO(&groups[i].write_fds);
                    continue;
                }
                log_message(
                    LOG_ERROR,
                    "event_loop",
                    std::string("select failed with WSA error ") + std::to_string(error)
                );
                return;
            }
            if (ready > 0 && FD_ISSET(impl_->wake_read, &groups[i].read_fds)) {
                wake_ready = true;
            }
        }

        if (wake_ready) {
            char buffer[256];
            while (recv(impl_->wake_read, buffer, sizeof(buffer), 0) > 0) {
            }
        }

        std::vector<ReadyCallback> callbacks;
        callbacks.reserve(active.size());
        for (std::size_t group = 0; group < groups.size(); ++group) {
            for (std::size_t i = 0; i < groups[group].watched.size(); ++i) {
                const SOCKET fd = groups[group].watched[i];
                int bits = 0;
                if (FD_ISSET(fd, &groups[group].read_fds)) {
                    bits |= IO_READ;
                }
                if (FD_ISSET(fd, &groups[group].write_fds)) {
                    bits |= IO_WRITE;
                }
                if (bits == 0) {
                    continue;
                }
                std::map<SOCKET, Impl::Entry>::const_iterator it = impl_->fds.find(fd);
                if (it == impl_->fds.end()) {
                    continue;
                }
                callbacks.push_back(ReadyCallback(it->second.on_events, bits));
            }
        }
        for (std::size_t i = 0; i < callbacks.size(); ++i) {
            callbacks[i].callback(callbacks[i].bits);
        }
    }
}

#endif
