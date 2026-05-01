#include "basic_mutex.h"

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

BasicLockGuard::BasicLockGuard(BasicMutex& mutex) : mutex_(mutex) {
    mutex_.lock();
}

BasicLockGuard::~BasicLockGuard() {
    mutex_.unlock();
}
