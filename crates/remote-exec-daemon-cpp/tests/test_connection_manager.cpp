#include "test_assert.h"
#include <atomic>
#include <utility>

#include "http/connection_transport.h"
#include "platform/platform.h"
#include "runtime/connection_manager.h"
#include "test_socket_pair.h"

static void hold_worker(std::atomic<bool>& release) {
    while (!release.load()) {
        platform::sleep_ms(10);
    }
}

static void assert_connection_limit_and_shutdown() {
    ConnectionManager manager(1UL);
    ConnectedSocketPair pair_one = make_connected_socket_pair();
    ConnectedSocketPair pair_two = make_connected_socket_pair();

    std::atomic<bool> release_first(false);

    TEST_ASSERT(manager.try_start(std::move(pair_one.first), [&release_first](SOCKET) {
        hold_worker(release_first);
    }));
    TEST_ASSERT(manager.active_count() == 1UL);
    TEST_ASSERT(!manager.try_start(std::move(pair_two.first), [&release_first](SOCKET) {
        hold_worker(release_first);
    }));

    manager.begin_shutdown();
    release_first.store(true);
    manager.wait_for_all();
    TEST_ASSERT(manager.active_count() == 0UL);
}

static void assert_completed_worker_socket_is_owned_until_retirement() {
    ConnectionManager manager(1UL);
    ConnectedSocketPair original = make_connected_socket_pair();
    const SOCKET socket = original.first.get();
    TEST_ASSERT(manager.try_start(std::move(original.first), [](SOCKET borrowed) {
        std::shared_ptr<ConnectionTransport> local = make_borrowed_connection_transport(borrowed);
        TEST_ASSERT(local->native_socket() == borrowed);
    }));
    const std::uint64_t deadline = platform::monotonic_ms() + 1000UL;
    while (manager.finished_count_for_testing() == 0UL && platform::monotonic_ms() < deadline) {
        platform::sleep_ms(1UL);
    }
    TEST_ASSERT(manager.finished_count_for_testing() == 1UL);
    int error = 0;
    TEST_ASSERT(socket_error_option(socket, &error) == 0);

    ConnectedSocketPair unrelated = make_connected_socket_pair();
    TEST_ASSERT(unrelated.first.get() != socket);
    TEST_ASSERT(unrelated.second.get() != socket);
    manager.begin_shutdown();
    const char expected = 'x';
    TEST_ASSERT(send_bounded(unrelated.first.get(), &expected, 1U, 0) == 1);
    TEST_ASSERT(wait_socket_readable(unrelated.second.get(), 1000UL) > 0);
    char received = '\0';
    TEST_ASSERT(recv_bounded(unrelated.second.get(), &received, 1U, 0) == 1);
    TEST_ASSERT(received == expected);
    manager.wait_for_all();
    TEST_ASSERT(socket_error_option(socket, &error) != 0);
}

static void assert_completion_between_reap_and_wait_is_observed() {
    ConnectionManager manager(1UL);
    ConnectedSocketPair pair = make_connected_socket_pair();
    std::atomic<bool> release(false);
    TEST_ASSERT(manager.try_start(std::move(pair.first), [&release](SOCKET) {
        hold_worker(release);
    }));
    manager.set_before_wait_hook_for_testing([&]() {
        release.store(true);
        const std::uint64_t deadline = platform::monotonic_ms() + 1000UL;
        while (manager.finished_count_for_testing() == 0UL && platform::monotonic_ms() < deadline) {
            platform::sleep_ms(1UL);
        }
        TEST_ASSERT(manager.finished_count_for_testing() == 1UL);
    });
    std::atomic<bool> returned(false);
    std::thread waiter([&]() {
        manager.wait_for_all();
        returned.store(true);
    });
    TEST_ASSERT(test_assert::wait_until_true(returned, 1000UL));
    waiter.join();
    TEST_ASSERT(manager.active_count() == 0UL);
}

int main() {
    assert_connection_limit_and_shutdown();
    assert_completed_worker_socket_is_owned_until_retirement();
    assert_completion_between_reap_and_wait_is_observed();
}
