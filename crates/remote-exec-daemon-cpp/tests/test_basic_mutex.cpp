#include "test_assert.h"
#include <atomic>
#include <cstdint>
#include <thread>
#include <vector>

#include "platform/basic_mutex.h"
#include "platform/platform.h"

static void wait_until_count(const std::atomic<int>& count, int expected) {
    const std::uint64_t deadline = platform::monotonic_ms() + 1000UL;
    while (count.load() < expected && platform::monotonic_ms() < deadline) {
        platform::sleep_ms(10UL);
    }
    TEST_ASSERT(count.load() == expected);
}

int main() {
    BasicMutex mutex;
    bool ready = false;
    std::atomic<int> waiting(0);

    {
        BasicCondVar cond;
        std::thread waiter([&]() {
            BasicLockGuard lock(mutex);
            ++waiting;
            while (!ready) {
                const bool woke = cond.timed_wait_ms(mutex, 500UL);
                TEST_ASSERT(woke);
            }
        });

        wait_until_count(waiting, 1);
        {
            BasicLockGuard lock(mutex);
            ready = true;
            cond.signal();
        }
        waiter.join();
    }

    {
        BasicCondVar cond;
        BasicLockGuard lock(mutex);
        const std::uint64_t start = platform::monotonic_ms();
        const bool woke = cond.timed_wait_ms(mutex, 75UL);
        const std::uint64_t elapsed = platform::monotonic_ms() - start;
        TEST_ASSERT(!woke);
        TEST_ASSERT(elapsed >= 50UL);
    }

    ready = false;
    waiting.store(0);
    int released = 0;
    {
        BasicCondVar cond;
        std::vector<std::thread> waiters;
        for (int i = 0; i < 2; ++i) {
            waiters.push_back(std::thread([&]() {
                BasicLockGuard lock(mutex);
                ++waiting;
                while (!ready) {
                    const bool woke = cond.timed_wait_ms(mutex, 500UL);
                    TEST_ASSERT(woke);
                }
                ++released;
            }));
        }
        wait_until_count(waiting, 2);
        {
            BasicLockGuard lock(mutex);
            ready = true;
            cond.broadcast();
        }
        for (std::size_t i = 0; i < waiters.size(); ++i) {
            waiters[i].join();
        }
    }
    TEST_ASSERT(released == 2);
}
