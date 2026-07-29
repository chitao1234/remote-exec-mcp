#pragma once

#include <functional>
#include <map>
#include <memory>
#include <thread>

#include "platform/basic_mutex.h"
#include "platform/socket.h"

class ConnectionTransport;

class ConnectionManager {
public:
    explicit ConnectionManager(unsigned long max_active_connections);
    ~ConnectionManager();

    // Owns one worker thread per accepted HTTP connection. begin_shutdown()
    // prevents new workers and asks active workers to stop; reap_finished() and
    // wait_for_all() are the only join paths. Worker code owns request handling,
    // while this manager supervises thread lifetime.
    bool try_start(UniqueSocket client, std::function<void(SOCKET)> worker_main);
    bool try_start_transport(
        const std::shared_ptr<ConnectionTransport>& client,
        std::function<void(const std::shared_ptr<ConnectionTransport>&)> worker_main
    );
    void begin_shutdown();
    void reap_finished();
    void wait_for_all();
    unsigned long active_count() const;

    ConnectionManager(const ConnectionManager&) = delete;
    ConnectionManager& operator=(const ConnectionManager&) = delete;

private:
    class ConnectionStartGate;
    struct WorkerRecord;
    void run_worker(const std::shared_ptr<WorkerRecord>& record);
    static void close_worker_socket(const std::shared_ptr<WorkerRecord>& record);
    static void shutdown_worker_socket(const std::shared_ptr<WorkerRecord>& record);
    static void join_worker_thread(const std::shared_ptr<WorkerRecord>& record);
    void erase_worker_record_locked(const std::shared_ptr<WorkerRecord>& record);
    // Spawns the worker thread for `record`; called under mutex_. On failure,
    // cleans up the record and returns false. On success, broadcasts
    // state_changed_ and returns true.
    bool launch_worker_locked(
        const std::shared_ptr<WorkerRecord>& record,
        const std::shared_ptr<ConnectionStartGate>& start_gate
    );

    unsigned long max_active_connections_;
    mutable BasicMutex mutex_;
    BasicCondVar state_changed_;
    std::map<unsigned long, std::shared_ptr<WorkerRecord>> workers_;
    bool shutting_down_;
    unsigned long next_worker_id_;
};
