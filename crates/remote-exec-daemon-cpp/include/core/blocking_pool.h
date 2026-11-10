#pragma once

#include <cstddef>
#include <deque>
#include <functional>
#include <thread>
#include <vector>

#include "platform/basic_mutex.h"

// Fixed worker pool for short blocking work (filesystem, archive, patch). Tasks
// must not join other tasks or block on a pool-owned resource.
class BlockingPool {
public:
    BlockingPool(std::size_t workers, std::size_t max_queue);
    ~BlockingPool();

    bool post(const std::function<void()>& task);
    void stop();

    BlockingPool(const BlockingPool&) = delete;
    BlockingPool& operator=(const BlockingPool&) = delete;

private:
    void worker_loop();

    BasicMutex mutex_;
    BasicCondVar cond_;
    std::deque<std::function<void()>> tasks_;
    std::vector<std::thread> threads_;
    std::size_t max_queue_;
    bool stopping_;
};
