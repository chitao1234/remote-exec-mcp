#pragma once

#include <atomic>
#include <cstdint>
#include <memory>
#include <string>
#include <thread>

#include "platform/basic_mutex.h"

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

    // Lifecycle owner: SessionStore owns LiveSession objects while they are
    // externally visible, and the output pump thread keeps a shared owner while
    // it drains process output. Terminal state is reached by retire_session(),
    // completed process output, or SessionStore destruction. The pump thread is
    // joined only after the session has been removed from the store or while
    // the store itself is shutting down.
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
    std::unique_ptr<std::thread> pump_thread_;
};
