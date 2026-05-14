#include <atomic>
#include "test_assert.h"
#include <utility>

#include "connection_manager.h"
#include "platform.h"
#include "test_socket_pair.h"

static void hold_worker(SOCKET socket, std::atomic<bool>& release) {
    while (!release.load()) {
        platform::sleep_ms(10);
    }
    close_socket(socket);
}

int main() {
    ConnectionManager manager(1UL);
    ConnectedSocketPair pair_one = make_connected_socket_pair();
    ConnectedSocketPair pair_two = make_connected_socket_pair();

    std::atomic<bool> release_first(false);

    TEST_ASSERT(manager.try_start(std::move(pair_one.first), [&release_first](SOCKET socket) {
        hold_worker(socket, release_first);
    }));
    TEST_ASSERT(manager.active_count() == 1UL);
    TEST_ASSERT(!manager.try_start(std::move(pair_two.first), [&release_first](SOCKET socket) {
        hold_worker(socket, release_first);
    }));

    manager.begin_shutdown();
    release_first.store(true);
    manager.wait_for_all();
    TEST_ASSERT(manager.active_count() == 0UL);
}
