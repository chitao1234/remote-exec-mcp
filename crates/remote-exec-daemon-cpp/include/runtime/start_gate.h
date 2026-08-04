#pragma once

#include "platform/basic_mutex.h"

// One-shot start gate: waiters block until release() (or cancel()) is called.
// release() unblocks waiters; cancel() additionally marks the gate cancelled
// so wait() reports false. Copying is disabled via the non-copyable mutex and
// condition variable members.
class StartGate {
public:
    StartGate() : released_(false), cancelled_(false) {}

    void release() {
        BasicLockGuard lock(mutex_);
        released_ = true;
        cond_.broadcast();
    }

    void cancel() {
        BasicLockGuard lock(mutex_);
        cancelled_ = true;
        released_ = true;
        cond_.broadcast();
    }

    bool wait() {
        BasicLockGuard lock(mutex_);
        while (!released_) {
            cond_.wait(mutex_);
        }
        return !cancelled_;
    }

private:
    BasicMutex mutex_;
    BasicCondVar cond_;
    bool released_;
    bool cancelled_;
};
