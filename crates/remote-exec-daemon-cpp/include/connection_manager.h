#pragma once

#include <functional>
#include <map>
#include <memory>

#ifdef _WIN32
#include <windows.h>
#include <winsock2.h>
#else
#include <thread>
#endif

#include "basic_mutex.h"
#include "server_transport.h"
#ifdef _WIN32
#include "win32_thread.h"
#endif

class ConnectionManager {
public:
    explicit ConnectionManager(unsigned long max_active_connections);
    ~ConnectionManager();

    // Owns one worker thread per accepted HTTP connection. begin_shutdown()
    // prevents new workers and shutdowns tracked sockets to wake blocking I/O;
    // reap_finished() and wait_for_all() are the only join paths. Worker code
    // owns request handling, but not the thread record that supervises it.
    bool try_start(UniqueSocket client, std::function<void(SOCKET)> worker_main);
    void begin_shutdown();
    void reap_finished();
    void wait_for_all();
    unsigned long active_count() const;

    ConnectionManager(const ConnectionManager&) = delete;
    ConnectionManager& operator=(const ConnectionManager&) = delete;

private:
    struct WorkerRecord;
    void run_worker(const std::shared_ptr<WorkerRecord>& record);
#ifdef _WIN32
    static unsigned __stdcall worker_thread_entry(void* raw_context);
#endif

    unsigned long max_active_connections_;
    mutable BasicMutex mutex_;
    BasicCondVar state_changed_;
    std::map<unsigned long, std::shared_ptr<WorkerRecord>> workers_;
    bool shutting_down_;
    unsigned long next_worker_id_;
};
