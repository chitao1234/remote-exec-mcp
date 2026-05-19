#pragma once

#include <atomic>
#include <cstdint>
#include <memory>
#include <string>
#include <thread>

#include "platform/basic_mutex.h"

class ProcessSession;

enum class SessionOutputDrainStopReason {
    None,
    OutputEof,
    StoreClosing,
    PumpError,
    IdleGraceExpired,
    MaxGraceExpired,
    DescendantTerminateUnsupported,
    DescendantTerminateTimeout,
};

const char* session_output_drain_stop_reason_name(SessionOutputDrainStopReason reason);

struct SessionOutputState {
    SessionOutputState();

    std::string buffered_output;
    std::string decode_carry;
    bool eof;
    bool exited;
    int exit_code;
    std::uint64_t generation;

    // Drain ownership lives with the session output state. Process backends
    // only answer whether descendants can be asked to close inherited output
    // handles; SessionStore and the output pump use this state to decide when
    // a parent-exited session is still publicly running.
    bool drain_started;
    std::uint64_t drain_started_at_ms;
    std::uint64_t drain_last_output_at_ms;
    bool descendant_cleanup_attempted;
    bool descendant_cleanup_supported;
    std::uint64_t descendant_cleanup_started_at_ms;
    SessionOutputDrainStopReason last_drain_stop_reason;
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
