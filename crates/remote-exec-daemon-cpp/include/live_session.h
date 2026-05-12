#pragma once

#include <atomic>
#include <cstdint>
#include <memory>
#include <string>

#ifdef _WIN32
#include <windows.h>
#include <winsock2.h>
#else
#include <thread>
#endif

#include "basic_mutex.h"

class ProcessSession;

struct SessionOutputState {
    SessionOutputState();

    std::string buffered_output;
    std::string decode_carry;
    bool eof;
    bool exited;
    int exit_code;
    std::uint64_t generation;
};

struct LiveSession {
    LiveSession();
    ~LiveSession();

    BasicMutex operation_mutex_;
    BasicMutex mutex_;
    BasicCondVar cond_;
    std::string id;
    std::unique_ptr<ProcessSession> process;
    std::uint64_t started_at_ms;
    std::atomic<std::uint64_t> last_touched_order;
    SessionOutputState output_;
    bool stdin_open;
    bool retired;
    bool closing;
    bool pump_started;
#ifdef _WIN32
    HANDLE pump_thread_;
#else
    std::unique_ptr<std::thread> pump_thread_;
#endif
};
