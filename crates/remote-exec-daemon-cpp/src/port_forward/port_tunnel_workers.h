#pragma once

#include <atomic>
#include <memory>
#include <thread>
#include <vector>

#ifdef _WIN32
#include <windows.h>
#endif

#include "platform/basic_mutex.h"
#include "port_tunnel_service.h"

struct PortTunnelService::WorkerGroup {
    WorkerGroup();

    struct Thread {
        Thread();

        std::atomic<bool> finished;
#ifdef _WIN32
        HANDLE handle;
        DWORD thread_id;
#else
        std::unique_ptr<std::thread> thread;
#endif
    };

    bool spawn(const std::shared_ptr<PortTunnelService>& service,
               const char* operation,
               PortTunnelWorkerLease worker_lease,
               const std::function<void()>& work);
    void begin_shutdown();
    void collect_finished(std::vector<std::shared_ptr<Thread>>* finished_workers);
    void join_workers(const std::vector<std::shared_ptr<Thread>>& workers);
    void join_all();

    BasicMutex mutex;
    std::vector<std::shared_ptr<Thread>> threads;
    bool shutting_down;
};
