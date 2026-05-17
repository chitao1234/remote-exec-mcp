#include "port_tunnel_thread.h"

#ifdef _WIN32
void consume_port_tunnel_thread(HANDLE* thread, DWORD thread_id) {
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
#else
void consume_port_tunnel_thread(std::unique_ptr<std::thread>* thread) {
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
#endif
