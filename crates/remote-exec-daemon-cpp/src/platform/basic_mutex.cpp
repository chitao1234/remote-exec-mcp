#include "basic_mutex.h"

BasicLockGuard::BasicLockGuard(BasicMutex& mutex) : mutex_(mutex) {
    mutex_.lock();
}

BasicLockGuard::~BasicLockGuard() {
    mutex_.unlock();
}
