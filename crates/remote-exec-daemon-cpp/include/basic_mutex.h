#pragma once

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <pthread.h>
#endif

class BasicMutex {
public:
    BasicMutex();
    ~BasicMutex();

    void lock();
    void unlock();

    BasicMutex(const BasicMutex&) = delete;
    BasicMutex& operator=(const BasicMutex&) = delete;

private:
    friend class BasicCondVar;
#ifdef _WIN32
    CRITICAL_SECTION mutex_;
#else
    pthread_mutex_t mutex_;
#endif
};

class BasicCondVar {
public:
    BasicCondVar();
    ~BasicCondVar();

    void wait(BasicMutex& mutex);
    bool timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms);
    void signal();
    void broadcast();

    BasicCondVar(const BasicCondVar&) = delete;
    BasicCondVar& operator=(const BasicCondVar&) = delete;

private:
#ifdef _WIN32
    HANDLE signal_event_;
    HANDLE broadcast_event_;
    long waiters_;
#else
    pthread_cond_t cond_;
#endif
};

class BasicLockGuard {
public:
    explicit BasicLockGuard(BasicMutex& mutex);
    ~BasicLockGuard();

    BasicLockGuard(const BasicLockGuard&) = delete;
    BasicLockGuard& operator=(const BasicLockGuard&) = delete;

private:
    BasicMutex& mutex_;
};
