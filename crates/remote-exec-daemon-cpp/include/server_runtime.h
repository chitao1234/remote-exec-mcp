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
#ifdef _WIN32
    static DWORD WINAPI accept_thread_entry(LPVOID raw_context);
#endif

    AppState state_;
    ConnectionManager connections_;
    mutable BasicMutex mutex_;
    UniqueSocket listener_;
    bool shutting_down_;
#ifdef _WIN32
    HANDLE accept_thread_;
#else
    std::thread* accept_thread_;
#endif
};
