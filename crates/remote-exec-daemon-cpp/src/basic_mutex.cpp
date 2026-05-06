#include "basic_mutex.h"

#ifndef _WIN32
#include <ctime>
#endif

BasicMutex::BasicMutex() {
#ifdef _WIN32
    InitializeCriticalSection(&mutex_);
#else
    pthread_mutex_init(&mutex_, NULL);
#endif
}

BasicMutex::~BasicMutex() {
#ifdef _WIN32
    DeleteCriticalSection(&mutex_);
#else
    pthread_mutex_destroy(&mutex_);
#endif
}

void BasicMutex::lock() {
#ifdef _WIN32
    EnterCriticalSection(&mutex_);
#else
    pthread_mutex_lock(&mutex_);
#endif
}

void BasicMutex::unlock() {
#ifdef _WIN32
    LeaveCriticalSection(&mutex_);
#else
    pthread_mutex_unlock(&mutex_);
#endif
}

BasicCondVar::BasicCondVar() {
#ifdef _WIN32
    signal_event_ = CreateEvent(NULL, FALSE, FALSE, NULL);
    broadcast_event_ = CreateEvent(NULL, TRUE, FALSE, NULL);
    waiters_ = 0;
#else
    pthread_cond_init(&cond_, NULL);
#endif
}

BasicCondVar::~BasicCondVar() {
#ifdef _WIN32
    if (signal_event_ != NULL) {
        CloseHandle(signal_event_);
    }
    if (broadcast_event_ != NULL) {
        CloseHandle(broadcast_event_);
    }
#else
    pthread_cond_destroy(&cond_);
#endif
}

void BasicCondVar::wait(BasicMutex& mutex) {
#ifdef _WIN32
    (void)timed_wait_ms(mutex, INFINITE);
#else
    pthread_cond_wait(&cond_, &mutex.mutex_);
#endif
}

bool BasicCondVar::timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms) {
#ifdef _WIN32
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
#else
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_sec += static_cast<time_t>(timeout_ms / 1000UL);
    deadline.tv_nsec += static_cast<long>((timeout_ms % 1000UL) * 1000000UL);
    if (deadline.tv_nsec >= 1000000000L) {
        ++deadline.tv_sec;
        deadline.tv_nsec -= 1000000000L;
    }
    return pthread_cond_timedwait(&cond_, &mutex.mutex_, &deadline) == 0;
#endif
}

void BasicCondVar::signal() {
#ifdef _WIN32
    if (InterlockedCompareExchange(&waiters_, 0, 0) > 0) {
        SetEvent(signal_event_);
    }
#else
    pthread_cond_signal(&cond_);
#endif
}

void BasicCondVar::broadcast() {
#ifdef _WIN32
    if (InterlockedCompareExchange(&waiters_, 0, 0) > 0) {
        SetEvent(broadcast_event_);
    }
#else
    pthread_cond_broadcast(&cond_);
#endif
}

BasicLockGuard::BasicLockGuard(BasicMutex& mutex) : mutex_(mutex) {
    mutex_.lock();
}

BasicLockGuard::~BasicLockGuard() {
    mutex_.unlock();
}
