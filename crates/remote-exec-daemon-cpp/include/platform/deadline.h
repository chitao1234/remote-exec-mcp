#pragma once

#include <climits>
#include <cstdint>
#include <limits>

#include "platform/platform.h"

namespace platform {

inline std::uint64_t monotonic_deadline_after_ms(unsigned long timeout_ms) {
    const std::uint64_t now = monotonic_ms();
    const std::uint64_t timeout = static_cast<std::uint64_t>(timeout_ms);
    const std::uint64_t max_deadline = std::numeric_limits<std::uint64_t>::max();
    return max_deadline - now < timeout ? max_deadline : now + timeout;
}

inline bool monotonic_deadline_expired(std::uint64_t deadline_ms) {
    return monotonic_ms() >= deadline_ms;
}

inline unsigned long monotonic_deadline_remaining_ms(std::uint64_t deadline_ms) {
    const std::uint64_t now = monotonic_ms();
    if (now >= deadline_ms) {
        return 0UL;
    }
    const std::uint64_t remaining = deadline_ms - now;
    if (remaining > static_cast<std::uint64_t>(ULONG_MAX)) {
        return ULONG_MAX;
    }
    return static_cast<unsigned long>(remaining);
}

inline unsigned long monotonic_deadline_remaining_ms_bounded(std::uint64_t deadline_ms, unsigned long max_ms) {
    const unsigned long remaining = monotonic_deadline_remaining_ms(deadline_ms);
    return remaining < max_ms ? remaining : max_ms;
}

inline int monotonic_deadline_remaining_ms_int(std::uint64_t deadline_ms) {
    const unsigned long remaining = monotonic_deadline_remaining_ms(deadline_ms);
    if (remaining > static_cast<unsigned long>(INT_MAX)) {
        return INT_MAX;
    }
    return static_cast<int>(remaining);
}

class MonotonicDeadline {
public:
    explicit MonotonicDeadline(unsigned long timeout_ms)
        : started_at_ms_(monotonic_ms()), timeout_ms_(timeout_ms),
          deadline_ms_(monotonic_deadline_after_ms(timeout_ms)) {}

    void reset_after(unsigned long timeout_ms) {
        started_at_ms_ = monotonic_ms();
        timeout_ms_ = timeout_ms;
        deadline_ms_ = monotonic_deadline_after_ms(timeout_ms);
    }

    bool expired() const { return monotonic_deadline_expired(deadline_ms_); }

    unsigned long remaining_ms() const { return monotonic_deadline_remaining_ms(deadline_ms_); }

    unsigned long remaining_ms_bounded(unsigned long max_ms) const {
        return monotonic_deadline_remaining_ms_bounded(deadline_ms_, max_ms);
    }

    int remaining_ms_int() const { return monotonic_deadline_remaining_ms_int(deadline_ms_); }

    std::uint64_t deadline_ms() const { return deadline_ms_; }

    std::uint64_t elapsed_ms() const { return monotonic_ms() - started_at_ms_; }

    unsigned long timeout_ms() const { return timeout_ms_; }

private:
    std::uint64_t started_at_ms_;
    unsigned long timeout_ms_;
    std::uint64_t deadline_ms_;
};

} // namespace platform
