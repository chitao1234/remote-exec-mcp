#include "runtime/connection_manager.h"

#include <exception>
#include <utility>
#include <vector>

#include "core/logging.h"
#include "http/connection_transport.h"
#include "runtime/daemon_thread.h"

struct ConnectionManager::WorkerRecord {
    WorkerRecord(
        unsigned long worker_id_value,
        std::function<void()> worker_main_value,
        const std::shared_ptr<ConnectionTransport>& transport_value
    )
        : worker_id(worker_id_value), worker_main(std::move(worker_main_value)),
          transport(transport_value), finished(false), thread() {}

    unsigned long worker_id;
    std::function<void()> worker_main;
    std::shared_ptr<ConnectionTransport> transport;
    // Protected by ConnectionManager::mutex_. The transport remains owned until
    // this record is retired and its worker has joined.
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
    record->worker_main();
    // Publish EOF promptly while retaining the descriptor until retirement.
    // Shutdown snapshots can safely retain this same owner in the meantime.
    shutdown_worker_socket(record);
    BasicLockGuard lock(mutex_);
    record->finished = true;
    state_changed_.broadcast();
}

void ConnectionManager::close_worker_socket(const std::shared_ptr<WorkerRecord>& record) {
    if (record->transport) {
        record->transport->shutdown();
    }
}

void ConnectionManager::shutdown_worker_socket(const std::shared_ptr<WorkerRecord>& record) {
    // Shutdown can run concurrently with I/O; physical close waits until the
    // worker and this record release the shared transport owner.
    close_worker_socket(record);
}

void ConnectionManager::join_worker_thread(const std::shared_ptr<WorkerRecord>& record) {
    join_daemon_thread(&record->thread);
}

void ConnectionManager::erase_worker_record_locked(const std::shared_ptr<WorkerRecord>& record) {
    workers_.erase(record->worker_id);
    state_changed_.broadcast();
}

bool ConnectionManager::launch_worker_locked(
    const std::shared_ptr<WorkerRecord>& record,
    const std::shared_ptr<ConnectionStartGate>& start_gate
) {
    try {
        record->thread.reset(new std::thread([this, record, start_gate]() {
            start_gate->wait();
            run_worker(record);
        }));
    } catch (const std::exception& ex) {
        log_message(
            LOG_WARN,
            "connection_manager",
            LogMessageBuilder("worker thread spawn failed")
                .field("worker_id", record->worker_id)
                .raw(std::string("error=") + ex.what())
                .str()
        );
        close_worker_socket(record);
        erase_worker_record_locked(record);
        return false;
    } catch (...) {
        log_message(
            LOG_WARN,
            "connection_manager",
            LogMessageBuilder("worker thread spawn failed")
                .field("worker_id", record->worker_id)
                .raw("error=unknown exception")
                .str()
        );
        close_worker_socket(record);
        erase_worker_record_locked(record);
        return false;
    }
    state_changed_.broadcast();
    return true;
}

bool ConnectionManager::try_start(UniqueSocket client, std::function<void(SOCKET)> worker_main) {
    return try_start_transport(
        make_plain_connection_transport(std::move(client)),
        [worker_main](const std::shared_ptr<ConnectionTransport>& transport) {
            worker_main(transport->native_socket());
        }
    );
}

bool ConnectionManager::try_start_transport(
    const std::shared_ptr<ConnectionTransport>& client,
    std::function<void(const std::shared_ptr<ConnectionTransport>&)> worker_main
) {
    std::shared_ptr<WorkerRecord> record;
    std::shared_ptr<ConnectionStartGate> start_gate(new ConnectionStartGate());
    {
        BasicLockGuard lock(mutex_);
        if (shutting_down_ || workers_.size() >= max_active_connections_) {
            return false;
        }
        const unsigned long worker_id = next_worker_id_++;
        record.reset(new WorkerRecord(
            worker_id,
            [client, worker_main]() { worker_main(client); },
            client
        ));
        workers_[worker_id] = record;
        if (!launch_worker_locked(record, start_gate)) {
            return false;
        }
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
        for (std::map<unsigned long, std::shared_ptr<WorkerRecord>>::const_iterator it =
                 workers_.begin();
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
            if (!it->second->finished) {
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
#ifdef REMOTE_EXEC_CPP_TESTING
        std::function<void()> hook;
        {
            BasicLockGuard lock(mutex_);
            hook.swap(before_wait_hook_);
        }
        if (hook) {
            hook();
        }
#endif
        BasicLockGuard lock(mutex_);
        if (workers_.empty()) {
            return;
        }
        bool has_finished = false;
        for (std::map<unsigned long, std::shared_ptr<WorkerRecord>>::const_iterator it =
                 workers_.begin();
             it != workers_.end();
             ++it) {
            has_finished = has_finished || it->second->finished;
        }
        if (has_finished) {
            continue;
        }
        state_changed_.wait(mutex_);
    }
}

unsigned long ConnectionManager::active_count() const {
    BasicLockGuard lock(mutex_);
    return static_cast<unsigned long>(workers_.size());
}

#ifdef REMOTE_EXEC_CPP_TESTING
void ConnectionManager::set_before_wait_hook_for_testing(const std::function<void()>& hook) {
    BasicLockGuard lock(mutex_);
    before_wait_hook_ = hook;
}

unsigned long ConnectionManager::finished_count_for_testing() const {
    BasicLockGuard lock(mutex_);
    unsigned long count = 0UL;
    for (std::map<unsigned long, std::shared_ptr<WorkerRecord>>::const_iterator it =
             workers_.begin();
         it != workers_.end();
         ++it) {
        if (it->second->finished) {
            ++count;
        }
    }
    return count;
}
#endif
