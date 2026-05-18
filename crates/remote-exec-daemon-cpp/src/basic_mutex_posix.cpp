#include "basic_mutex.h"

#include <ctime>

#if defined(CLOCK_MONOTONIC) && defined(__GNUC__) && !defined(__APPLE__)
extern "C" int pthread_condattr_setclock(pthread_condattr_t* attr, clockid_t clock_id) __attribute__((weak));
#endif

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

namespace {

bool pthread_condattr_setclock_available() {
#if defined(CLOCK_MONOTONIC) && defined(__GNUC__) && !defined(__APPLE__)
    return pthread_condattr_setclock != nullptr;
#else
    return false;
#endif
}

bool init_condvar_monotonic(pthread_cond_t* cond) {
#if !defined(CLOCK_MONOTONIC)
    pthread_cond_init(cond, nullptr);
    return false;
#else
    if (!pthread_condattr_setclock_available()) {
        pthread_cond_init(cond, nullptr);
        return false;
    }
    pthread_condattr_t attr;
    if (pthread_condattr_init(&attr) != 0) {
        pthread_cond_init(cond, nullptr);
        return false;
    }
    if (pthread_condattr_setclock(&attr, CLOCK_MONOTONIC) != 0) {
        pthread_condattr_destroy(&attr);
        pthread_cond_init(cond, nullptr);
        return false;
    }
    pthread_cond_init(cond, &attr);
    pthread_condattr_destroy(&attr);
    return true;
#endif
}

clockid_t condvar_deadline_clock(bool uses_monotonic) {
#ifdef CLOCK_MONOTONIC
    if (uses_monotonic) {
        return CLOCK_MONOTONIC;
    }
#else
    (void)uses_monotonic;
#endif
    return CLOCK_REALTIME;
}

} // namespace

BasicCondVar::BasicCondVar() : uses_monotonic_(init_condvar_monotonic(&cond_)) {}

BasicCondVar::~BasicCondVar() {
    pthread_cond_destroy(&cond_);
}

void BasicCondVar::wait(BasicMutex& mutex) {
    pthread_cond_wait(&cond_, &mutex.mutex_);
}

bool BasicCondVar::timed_wait_ms(BasicMutex& mutex, unsigned long timeout_ms) {
    struct timespec deadline;
    clock_gettime(condvar_deadline_clock(uses_monotonic_), &deadline);
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
