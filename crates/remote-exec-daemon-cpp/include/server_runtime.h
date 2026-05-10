#pragma once

#include "connection_manager.h"
#include "server.h"

class ServerRuntime {
public:
    explicit ServerRuntime(const DaemonConfig& config);
    ~ServerRuntime();

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
#ifdef _WIN32
    static unsigned __stdcall accept_thread_entry(void* raw_context);
    static unsigned __stdcall maintenance_thread_entry(void* raw_context);
#endif

    AppState state_;
    ConnectionManager connections_;
    mutable BasicMutex mutex_;
    UniqueSocket listener_;
    bool shutting_down_;
#ifdef _WIN32
    HANDLE accept_thread_;
    HANDLE maintenance_thread_;
#else
    std::thread* accept_thread_;
    std::thread* maintenance_thread_;
#endif
};
