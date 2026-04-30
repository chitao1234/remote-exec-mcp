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
#ifdef _WIN32
    CRITICAL_SECTION mutex_;
#else
    pthread_mutex_t mutex_;
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
