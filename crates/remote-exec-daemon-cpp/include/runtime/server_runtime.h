#pragma once

#include <atomic>
#include <memory>
#include <string>
#include <thread>

#include "platform/wakeup_pipe.h"
#include "runtime/app_state.h"
#include "runtime/connection_manager.h"

class ServerRuntime {
public:
    explicit ServerRuntime(const DaemonConfig& config);
    ~ServerRuntime();

    // Top-level daemon runtime owner. It owns daemon config, route metadata,
    // sandbox state, services, the listener socket, accept and maintenance
    // threads, and the HTTP ConnectionManager. request_shutdown() closes the
    // listener and asks child owners to stop; join() consumes runtime threads
    // and waits for connection workers before final maintenance.
    void start_accept_loop();
    void start_reverse_loop();
    void request_shutdown();
    void join();
    unsigned short bound_port() const;
    const DaemonConfig& config() const;
    const AppMetadata& metadata() const;
    ConnectionManager& connection_manager();
    void maintenance_once();

    ServerRuntime(const ServerRuntime&) = delete;
    ServerRuntime& operator=(const ServerRuntime&) = delete;

private:
    void accept_loop();
    void reverse_loop();
    void maintenance_loop();

    DaemonConfig config_;
    AppMetadata metadata_;
    AppSandboxState sandbox_;
    AppServices services_;
    AppShutdownState shutdown_;
    ConnectionManager connections_;
    mutable BasicMutex mutex_;
    UniqueSocket listener_;
    WakeupPipe shutdown_wakeup_;
    bool shutting_down_;
    std::unique_ptr<std::thread> accept_thread_;
    std::unique_ptr<std::thread> maintenance_thread_;
    std::atomic<unsigned long> reverse_live_;
    std::atomic<unsigned long> reverse_busy_;
};
