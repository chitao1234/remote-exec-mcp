#include "basic_mutex.h"

BasicMutex::BasicMutex() {
    InitializeCriticalSection(&mutex_);
}

BasicMutex::~BasicMutex() {
    DeleteCriticalSection(&mutex_);
}

void BasicMutex::lock() {
    EnterCriticalSection(&mutex_);
}

void BasicMutex::unlock() {
    LeaveCriticalSection(&mutex_);
}

BasicCondVar::BasicCondVar() {
    signal_event_ = CreateEvent(nullptr, FALSE, FALSE, nullptr);
    broadcast_event_ = CreateEvent(nullptr, TRUE, FALSE, nullptr);
    waiters_ = 0;
}

BasicCondVar::~BasicCondVar() {
    if (signal_event_ != nullptr) {
        CloseHandle(signal_event_);
    }
    if (broadcast_event_ != nullptr) {
        CloseHandle(broadcast_event_);
    }
}

void BasicCondVar::wait(BasicMutex& mutex) {
    (void)timed_wait_ms(mutex, INFINITE);
}

bool BasicCondVar::timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms) {
    InterlockedIncrement(&waiters_);
    mutex.unlock();
    const HANDLE handles[2] = {signal_event_, broadcast_event_};
    const DWORD result = WaitForMultipleObjects(2, handles, FALSE, timeout_ms);
    mutex.lock();
    const long remaining = InterlockedDecrement(&waiters_);
    if (result == WAIT_OBJECT_0 + 1 && remaining == 0) {
        ResetEvent(broadcast_event_);
    }
    return result == WAIT_OBJECT_0 || result == WAIT_OBJECT_0 + 1;
}

void BasicCondVar::signal() {
    // The Win32 emulation uses an auto-reset event for signal and a manual-reset
    // event for broadcast. Peek waiters_ first so signal/broadcast stay no-ops
    // when nobody is waiting; the last waiter released by broadcast resets the
    // manual-reset event in timed_wait_ms().
    if (InterlockedCompareExchange(&waiters_, 0, 0) > 0) {
        SetEvent(signal_event_);
    }
}

void BasicCondVar::broadcast() {
    if (InterlockedCompareExchange(&waiters_, 0, 0) > 0) {
        SetEvent(broadcast_event_);
    }
}
