#pragma once

#include <atomic>
#include <memory>
#include <thread>
#include <vector>

#include "platform/basic_mutex.h"
#include "port_tunnel_service.h"

struct PortTunnelService::WorkerGroup {
    WorkerGroup();

    struct Thread {
        Thread();

        std::atomic<bool> finished;
        std::unique_ptr<std::thread> thread;
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
