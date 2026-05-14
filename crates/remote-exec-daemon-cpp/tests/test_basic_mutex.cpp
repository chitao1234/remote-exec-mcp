#include "test_assert.h"
#include <cstdint>
#include <thread>
#include <vector>

#include "basic_mutex.h"
#include "platform.h"

int main() {
    BasicMutex mutex;
    bool ready = false;

    {
        BasicCondVar cond;
        std::thread waiter([&]() {
            BasicLockGuard lock(mutex);
            while (!ready) {
                const bool woke = cond.timed_wait_ms(mutex, 500UL);
                TEST_ASSERT(woke);
            }
        });

        platform::sleep_ms(50);
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
    int released = 0;
    {
        BasicCondVar cond;
        std::vector<std::thread> waiters;
        for (int i = 0; i < 2; ++i) {
            waiters.push_back(std::thread([&]() {
                BasicLockGuard lock(mutex);
                while (!ready) {
                    const bool woke = cond.timed_wait_ms(mutex, 500UL);
                    TEST_ASSERT(woke);
                }
                ++released;
            }));
        }
        platform::sleep_ms(50);
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
