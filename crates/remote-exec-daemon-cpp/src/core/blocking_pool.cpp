#include "core/blocking_pool.h"

#include <exception>
#include <string>
#include <utility>

#include "core/logging.h"

BlockingPool::BlockingPool(std::size_t workers, std::size_t max_queue)
    : max_queue_(max_queue == 0U ? 1U : max_queue), stopping_(false) {
    if (workers == 0U) {
        workers = 1U;
    }
    threads_.reserve(workers);
    try {
        for (std::size_t i = 0; i < workers; ++i) {
            threads_.push_back(std::thread(&BlockingPool::worker_loop, this));
        }
    } catch (...) {
        {
            BasicLockGuard lock(mutex_);
            stopping_ = true;
        }
        cond_.broadcast();
        for (std::size_t i = 0; i < threads_.size(); ++i) {
            if (threads_[i].joinable()) {
                threads_[i].join();
            }
        }
        throw;
    }
}

BlockingPool::~BlockingPool() {
    stop();
}

bool BlockingPool::post(const std::function<void()>& task) {
    {
        BasicLockGuard lock(mutex_);
        if (stopping_ || tasks_.size() >= max_queue_) {
            return false;
        }
        tasks_.push_back(task);
    }
    cond_.signal();
    return true;
}

void BlockingPool::stop() {
    {
        BasicLockGuard lock(mutex_);
        if (stopping_) {
            return;
        }
        stopping_ = true;
    }
    cond_.broadcast();
    for (std::size_t i = 0; i < threads_.size(); ++i) {
        if (threads_[i].joinable()) {
            threads_[i].join();
        }
    }
    threads_.clear();
}

void BlockingPool::worker_loop() {
    for (;;) {
        std::function<void()> task;
        {
            BasicLockGuard lock(mutex_);
            while (!stopping_ && tasks_.empty()) {
                cond_.wait(mutex_);
            }
            if (tasks_.empty()) {
                return;
            }
            task = tasks_.front();
            tasks_.pop_front();
        }

        try {
            task();
        } catch (const std::exception& ex) {
            log_message(LOG_ERROR, "blocking_pool", std::string("task failed: ") + ex.what());
        } catch (...) {
            log_message(LOG_ERROR, "blocking_pool", "task failed with an unknown exception");
        }
    }
}
