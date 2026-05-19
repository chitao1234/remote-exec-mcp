#pragma once

#include <memory>
#include <thread>

#include "runtime/connection_manager.h"
#include "runtime/server.h"
#include "platform/wakeup_pipe.h"

class ServerRuntime {
public:
    explicit ServerRuntime(const DaemonConfig& config);
    ~ServerRuntime();

    // Top-level daemon runtime owner. It owns AppState, the listener socket, the
    // accept and maintenance threads, the HTTP ConnectionManager, and the
    // PortTunnelService stored in AppState. request_shutdown() closes the
    // listener and asks child owners to stop; join() consumes runtime threads and
    // waits for connection workers before final maintenance.
    void start_accept_loop();
    void request_shutdown();
    void join();
    unsigned short bound_port() const;
    AppState& state();
    ConnectionManager& connection_manager();
    void maintenance_once();

    ServerRuntime(const ServerRuntime&) = delete;
    ServerRuntime& operator=(const ServerRuntime&) = delete;

private:
    void accept_loop();
    void maintenance_loop();

    AppState state_;
    ConnectionManager connections_;
    mutable BasicMutex mutex_;
    UniqueSocket listener_;
    WakeupPipe shutdown_wakeup_;
    bool shutting_down_;
    std::unique_ptr<std::thread> accept_thread_;
    std::unique_ptr<std::thread> maintenance_thread_;
};
