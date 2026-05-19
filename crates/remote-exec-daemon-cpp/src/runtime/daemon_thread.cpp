#include "runtime/daemon_thread.h"

void consume_daemon_thread(std::unique_ptr<std::thread>* thread) {
    std::unique_ptr<std::thread> owned;
    owned.swap(*thread);
    if (owned.get() == nullptr) {
        return;
    }
    if (owned->get_id() == std::this_thread::get_id()) {
        owned->detach();
        return;
    }
    owned->join();
}

void join_daemon_thread(std::unique_ptr<std::thread>* thread) {
    consume_daemon_thread(thread);
}
