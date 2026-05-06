#pragma once

#include <map>
#include <memory>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <thread>
#endif

#include "basic_mutex.h"
#include "server_transport.h"

typedef void (*ConnectionWorkerMain)(SOCKET socket, void* context);

class ConnectionManager {
public:
    explicit ConnectionManager(unsigned long max_active_connections);
    ~ConnectionManager();

    bool try_start(UniqueSocket client, ConnectionWorkerMain worker_main, void* context);
    void begin_shutdown();
    void reap_finished();
    unsigned long active_count() const;

    ConnectionManager(const ConnectionManager&) = delete;
    ConnectionManager& operator=(const ConnectionManager&) = delete;

private:
    struct WorkerRecord;
    static void run_worker(const std::shared_ptr<WorkerRecord>& record);
#ifdef _WIN32
    static DWORD WINAPI worker_thread_entry(LPVOID raw_context);
#endif

    unsigned long max_active_connections_;
    mutable BasicMutex mutex_;
    std::map<unsigned long, std::shared_ptr<WorkerRecord> > workers_;
    bool shutting_down_;
    unsigned long next_worker_id_;
};
