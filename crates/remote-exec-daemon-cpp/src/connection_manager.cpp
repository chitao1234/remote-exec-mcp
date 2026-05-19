#include "connection_manager.h"

#include <vector>

#include "daemon_thread.h"

namespace {

class ConnectionStartGate {
public:
    ConnectionStartGate() : released_(false) {}

    void release() {
        BasicLockGuard lock(mutex_);
        released_ = true;
        cond_.broadcast();
    }

    void wait() {
        BasicLockGuard lock(mutex_);
        while (!released_) {
            cond_.wait(mutex_);
        }
    }

private:
    BasicMutex mutex_;
    BasicCondVar cond_;
    bool released_;
};

} // namespace

struct ConnectionManager::WorkerRecord {
    WorkerRecord(unsigned long worker_id_value,
                 SOCKET socket_value,
                 std::function<void(SOCKET)> worker_main_value)
        : worker_id(worker_id_value), socket(socket_value), worker_main(std::move(worker_main_value)), finished(false)
#ifdef _WIN32
          ,
          thread_handle(nullptr)
#else
          ,
          thread()
#endif
    {
    }

    unsigned long worker_id;
    SOCKET socket;
    std::function<void(SOCKET)> worker_main;
    BasicMutex state_mutex;
    bool finished;
#ifdef _WIN32
    HANDLE thread_handle;
#else
    std::unique_ptr<std::thread> thread;
#endif
};

ConnectionManager::ConnectionManager(unsigned long max_active_connections)
    : max_active_connections_(max_active_connections), shutting_down_(false), next_worker_id_(1UL) {
}

ConnectionManager::~ConnectionManager() {
    begin_shutdown();
    wait_for_all();
}

void ConnectionManager::run_worker(const std::shared_ptr<WorkerRecord>& record) {
    record->worker_main(record->socket);
    {
        BasicLockGuard lock(record->state_mutex);
        record->socket = INVALID_SOCKET;
        record->finished = true;
    }
    BasicLockGuard lock(mutex_);
    state_changed_.broadcast();
}

#ifdef _WIN32
struct ConnectionManager::WorkerContext {
    ConnectionManager* manager;
    std::shared_ptr<WorkerRecord> record;
    std::shared_ptr<ConnectionStartGate> start_gate;
};

unsigned __stdcall ConnectionManager::worker_thread_entry(void* raw_context) {
    std::unique_ptr<WorkerContext> context(static_cast<WorkerContext*>(raw_context));
    context->start_gate->wait();
    context->manager->run_worker(context->record);
    return 0;
}
#endif

bool ConnectionManager::try_start(UniqueSocket client, std::function<void(SOCKET)> worker_main) {
    std::shared_ptr<WorkerRecord> record;
    std::shared_ptr<ConnectionStartGate> start_gate(new ConnectionStartGate());
    {
        BasicLockGuard lock(mutex_);
        if (shutting_down_ || workers_.size() >= max_active_connections_) {
            return false;
        }
        const unsigned long worker_id = next_worker_id_++;
        record.reset(new WorkerRecord(worker_id, client.get(), std::move(worker_main)));
        workers_[worker_id] = record;
        client.release();
#ifndef _WIN32
        try {
            record->thread.reset(new std::thread([this, record, start_gate]() {
                start_gate->wait();
                run_worker(record);
            }));
        } catch (...) {
            {
                BasicLockGuard state_lock(record->state_mutex);
                if (record->socket != INVALID_SOCKET) {
                    close_socket(record->socket);
                    record->socket = INVALID_SOCKET;
                }
            }
            workers_.erase(record->worker_id);
            state_changed_.broadcast();
            return false;
        }
#endif
        state_changed_.broadcast();
    }

#ifdef _WIN32
    std::unique_ptr<WorkerContext> thread_context(new WorkerContext());
    thread_context->manager = this;
    thread_context->record = record;
    thread_context->start_gate = start_gate;
    HANDLE handle = begin_win32_thread(&ConnectionManager::worker_thread_entry, thread_context.get());
    if (handle == nullptr) {
        {
            BasicLockGuard state_lock(record->state_mutex);
            if (record->socket != INVALID_SOCKET) {
                close_socket(record->socket);
                record->socket = INVALID_SOCKET;
            }
        }
        BasicLockGuard lock(mutex_);
        workers_.erase(record->worker_id);
        state_changed_.broadcast();
        return false;
    }
    record->thread_handle = handle;
    thread_context.release();
    start_gate->release();
#else
    start_gate->release();
#endif
    return true;
}

void ConnectionManager::begin_shutdown() {
    std::vector<std::shared_ptr<WorkerRecord>> snapshot;
    {
        BasicLockGuard lock(mutex_);
        shutting_down_ = true;
        state_changed_.broadcast();
        for (std::map<unsigned long, std::shared_ptr<WorkerRecord>>::const_iterator it = workers_.begin();
             it != workers_.end();
             ++it) {
            snapshot.push_back(it->second);
        }
    }

    for (std::size_t i = 0; i < snapshot.size(); ++i) {
        BasicLockGuard state_lock(snapshot[i]->state_mutex);
        if (snapshot[i]->socket != INVALID_SOCKET) {
            shutdown_socket(snapshot[i]->socket);
        }
    }
}

void ConnectionManager::reap_finished() {
    std::vector<std::shared_ptr<WorkerRecord>> finished;
    {
        BasicLockGuard lock(mutex_);
        for (std::map<unsigned long, std::shared_ptr<WorkerRecord>>::iterator it = workers_.begin();
             it != workers_.end();) {
            bool done = false;
            {
                BasicLockGuard state_lock(it->second->state_mutex);
                done = it->second->finished;
            }
            if (!done) {
                ++it;
                continue;
            }
            finished.push_back(it->second);
            workers_.erase(it++);
            state_changed_.broadcast();
        }
    }

    for (std::size_t i = 0; i < finished.size(); ++i) {
#ifdef _WIN32
        join_daemon_thread(&finished[i]->thread_handle);
#else
        join_daemon_thread(&finished[i]->thread);
#endif
    }
}

void ConnectionManager::wait_for_all() {
    for (;;) {
        reap_finished();
        BasicLockGuard lock(mutex_);
        if (workers_.empty()) {
            return;
        }
        state_changed_.wait(mutex_);
    }
}

unsigned long ConnectionManager::active_count() const {
    BasicLockGuard lock(mutex_);
    return static_cast<unsigned long>(workers_.size());
}
