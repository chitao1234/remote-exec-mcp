#include "runtime/connection_manager.h"

#include <exception>
#include <vector>

#include "core/logging.h"
#include "runtime/daemon_thread.h"

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
        : worker_id(worker_id_value), socket(socket_value), worker_main(std::move(worker_main_value)), finished(false),
          thread()
    {
    }

    unsigned long worker_id;
    SOCKET socket;
    std::function<void(SOCKET)> worker_main;
    BasicMutex state_mutex;
    bool finished;
    std::unique_ptr<std::thread> thread;
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

void ConnectionManager::close_worker_socket(const std::shared_ptr<WorkerRecord>& record) {
    BasicLockGuard state_lock(record->state_mutex);
    if (record->socket != INVALID_SOCKET) {
        close_socket(record->socket);
        record->socket = INVALID_SOCKET;
    }
}

void ConnectionManager::shutdown_worker_socket(const std::shared_ptr<WorkerRecord>& record) {
    BasicLockGuard state_lock(record->state_mutex);
    if (record->socket != INVALID_SOCKET) {
        shutdown_socket(record->socket);
    }
}

void ConnectionManager::join_worker_thread(const std::shared_ptr<WorkerRecord>& record) {
    join_daemon_thread(&record->thread);
}

void ConnectionManager::erase_worker_record_locked(const std::shared_ptr<WorkerRecord>& record) {
    workers_.erase(record->worker_id);
    state_changed_.broadcast();
}

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
        try {
            record->thread.reset(new std::thread([this, record, start_gate]() {
                start_gate->wait();
                run_worker(record);
            }));
        } catch (const std::exception& ex) {
            log_message(LOG_WARN,
                        "connection_manager",
                        LogMessageBuilder("worker thread spawn failed")
                            .field("worker_id", record->worker_id)
                            .raw(std::string("error=") + ex.what())
                            .str());
            close_worker_socket(record);
            erase_worker_record_locked(record);
            return false;
        } catch (...) {
            log_message(LOG_WARN,
                        "connection_manager",
                        LogMessageBuilder("worker thread spawn failed")
                            .field("worker_id", record->worker_id)
                            .raw("error=unknown exception")
                            .str());
            close_worker_socket(record);
            erase_worker_record_locked(record);
            return false;
        }
        state_changed_.broadcast();
    }

    start_gate->release();
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
        shutdown_worker_socket(snapshot[i]);
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
        join_worker_thread(finished[i]);
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
