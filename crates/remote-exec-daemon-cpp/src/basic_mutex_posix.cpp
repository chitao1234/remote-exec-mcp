#include "basic_mutex.h"

#include <ctime>

BasicMutex::BasicMutex() {
    pthread_mutex_init(&mutex_, nullptr);
}

BasicMutex::~BasicMutex() {
    pthread_mutex_destroy(&mutex_);
}

void BasicMutex::lock() {
    pthread_mutex_lock(&mutex_);
}

void BasicMutex::unlock() {
    pthread_mutex_unlock(&mutex_);
}

BasicCondVar::BasicCondVar() {
    pthread_cond_init(&cond_, nullptr);
}

BasicCondVar::~BasicCondVar() {
    pthread_cond_destroy(&cond_);
}

void BasicCondVar::wait(BasicMutex& mutex) {
    pthread_cond_wait(&cond_, &mutex.mutex_);
}

bool BasicCondVar::timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms) {
    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_sec += static_cast<time_t>(timeout_ms / 1000UL);
    deadline.tv_nsec += static_cast<long>((timeout_ms % 1000UL) * 1000000UL);
    if (deadline.tv_nsec >= 1000000000L) {
        ++deadline.tv_sec;
        deadline.tv_nsec -= 1000000000L;
    }
    return pthread_cond_timedwait(&cond_, &mutex.mutex_, &deadline) == 0;
}

void BasicCondVar::signal() {
    pthread_cond_signal(&cond_);
}

void BasicCondVar::broadcast() {
    pthread_cond_broadcast(&cond_);
}
