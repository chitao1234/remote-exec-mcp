#pragma once

#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>

#include "platform/socket.h"

enum EventInterest {
    IO_READ = 1,
    IO_WRITE = 2,
};

// Single-owner readiness loop. Descriptor state (add/remove/interest) is only
// touched by the loop thread; post(), stop(), and timer cancellation are safe
// from other threads.
class EventLoop {
public:
    typedef std::uint64_t TimerId;

    // capacity is the maximum number of watched sockets. Zero selects the
    // backend default; the Windows backend chunks large loops across multiple
    // select() sets so callers may request more than FD_SETSIZE.
    explicit EventLoop(std::size_t capacity = 0);
    ~EventLoop();

    bool valid() const;

    // Socket count this backend can watch, excluding the loop's own wakeup.
    static std::size_t max_watched_sockets();

    bool add(SOCKET fd, int interest, const std::function<void(int)>& on_events);
    bool set_interest(SOCKET fd, int interest);
    void remove(SOCKET fd);
    void post(const std::function<void()>& fn);
    TimerId add_timer(std::uint64_t deadline_ms, const std::function<void()>& fn);
    void cancel_timer(TimerId id);
    void run();
    void stop();

    EventLoop(const EventLoop&) = delete;
    EventLoop& operator=(const EventLoop&) = delete;

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};
