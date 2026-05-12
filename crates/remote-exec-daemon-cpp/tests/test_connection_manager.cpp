#include <atomic>
#include <cassert>
#include <utility>

#include "connection_manager.h"
#include "platform.h"
#include "test_socket_pair.h"

static void hold_worker(SOCKET socket, void* raw_flag) {
    std::atomic<bool>* release = static_cast<std::atomic<bool>*>(raw_flag);
    while (!release->load()) {
        platform::sleep_ms(10);
    }
    close_socket(socket);
}

int main() {
    ConnectionManager manager(1UL);
    ConnectedSocketPair pair_one = make_connected_socket_pair();
    ConnectedSocketPair pair_two = make_connected_socket_pair();

    std::atomic<bool> release_first(false);

    assert(manager.try_start(std::move(pair_one.first), &hold_worker, &release_first));
    assert(manager.active_count() == 1UL);
    assert(!manager.try_start(std::move(pair_two.first), &hold_worker, &release_first));

    manager.begin_shutdown();
    release_first.store(true);
    manager.wait_for_all();
    assert(manager.active_count() == 0UL);
}
