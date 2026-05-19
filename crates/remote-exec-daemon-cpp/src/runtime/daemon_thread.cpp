#include "runtime/daemon_thread.h"

#ifdef _WIN32
void consume_daemon_thread(HANDLE* thread, DWORD thread_id) {
    HANDLE handle = *thread;
    *thread = nullptr;
    if (handle == nullptr) {
        return;
    }
    if (thread_id == 0U || thread_id != GetCurrentThreadId()) {
        WaitForSingleObject(handle, INFINITE);
    }
    CloseHandle(handle);
}

void join_daemon_thread(HANDLE* thread) {
    HANDLE handle = *thread;
    *thread = nullptr;
    if (handle == nullptr) {
        return;
    }
    WaitForSingleObject(handle, INFINITE);
    CloseHandle(handle);
}
#else
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
#endif
