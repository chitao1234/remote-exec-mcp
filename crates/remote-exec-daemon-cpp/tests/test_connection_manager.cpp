#include <cassert>
#include <thread>

#include <unistd.h>

#include <sys/socket.h>

#include "connection_manager.h"
#include "platform.h"

static void hold_worker(SOCKET socket, void* raw_flag) {
    bool* release = static_cast<bool*>(raw_flag);
    while (!*release) {
        platform::sleep_ms(10);
    }
    close(socket);
}

int main() {
    ConnectionManager manager(1UL);
    int pair_one[2];
    int pair_two[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair_one) == 0);
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, pair_two) == 0);

    bool release_first = false;
    UniqueSocket first(pair_one[0]);
    UniqueSocket second(pair_two[0]);

    assert(manager.try_start(std::move(first), &hold_worker, &release_first));
    assert(manager.active_count() == 1UL);
    assert(!manager.try_start(std::move(second), &hold_worker, &release_first));

    manager.begin_shutdown();
    release_first = true;
    manager.reap_finished();
}
