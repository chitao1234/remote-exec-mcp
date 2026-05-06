#include "connection_manager.h"

#include <vector>

struct ConnectionManager::WorkerRecord {
    WorkerRecord(
        unsigned long worker_id_value,
        SOCKET socket_value,
        ConnectionWorkerMain worker_main_value,
        void* context_value
    )
        : worker_id(worker_id_value),
          socket(socket_value),
          worker_main(worker_main_value),
          context(context_value),
          finished(false)
#ifdef _WIN32
          ,
          thread_handle(NULL)
#else
          ,
          thread(NULL)
#endif
    {
    }

    unsigned long worker_id;
    SOCKET socket;
    ConnectionWorkerMain worker_main;
    void* context;
    BasicMutex state_mutex;
    bool finished;
#ifdef _WIN32
    HANDLE thread_handle;
#else
    std::thread* thread;
#endif
};

namespace {

#ifdef _WIN32
struct WorkerThreadContext {
    std::shared_ptr<ConnectionManager::WorkerRecord> record;
};

DWORD WINAPI worker_thread_entry(LPVOID raw_context) {
    std::unique_ptr<WorkerThreadContext> context(
        static_cast<WorkerThreadContext*>(raw_context)
    );
    ConnectionManager::run_worker(context->record);
    return 0;
}
#endif

}  // namespace

ConnectionManager::ConnectionManager(unsigned long max_active_connections)
    : max_active_connections_(max_active_connections),
      shutting_down_(false),
      next_worker_id_(1UL) {}

ConnectionManager::~ConnectionManager() {
    begin_shutdown();
    reap_finished();
}

void ConnectionManager::run_worker(const std::shared_ptr<WorkerRecord>& record) {
    record->worker_main(record->socket, record->context);
    BasicLockGuard lock(record->state_mutex);
    record->socket = INVALID_SOCKET;
    record->finished = true;
}

bool ConnectionManager::try_start(
    UniqueSocket client,
    ConnectionWorkerMain worker_main,
    void* context
) {
    std::shared_ptr<WorkerRecord> record;
    {
        BasicLockGuard lock(mutex_);
        if (shutting_down_ || workers_.size() >= max_active_connections_) {
            return false;
        }
        const unsigned long worker_id = next_worker_id_++;
        record.reset(new WorkerRecord(worker_id, client.release(), worker_main, context));
        workers_[worker_id] = record;
    }

#ifdef _WIN32
    std::unique_ptr<WorkerThreadContext> thread_context(new WorkerThreadContext());
    thread_context->record = record;
    HANDLE handle = CreateThread(NULL, 0, worker_thread_entry, thread_context.get(), 0, NULL);
    if (handle == NULL) {
        close_socket(record->socket);
        BasicLockGuard lock(mutex_);
        workers_.erase(record->worker_id);
        return false;
    }
    record->thread_handle = handle;
    thread_context.release();
#else
    record->thread = new std::thread(run_worker, record);
#endif
    return true;
}

void ConnectionManager::begin_shutdown() {
    std::vector<std::shared_ptr<WorkerRecord> > snapshot;
    {
        BasicLockGuard lock(mutex_);
        shutting_down_ = true;
        for (std::map<unsigned long, std::shared_ptr<WorkerRecord> >::const_iterator it =
                 workers_.begin();
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
    std::vector<std::shared_ptr<WorkerRecord> > finished;
    {
        BasicLockGuard lock(mutex_);
        for (std::map<unsigned long, std::shared_ptr<WorkerRecord> >::iterator it =
                 workers_.begin();
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
        }
    }

    for (std::size_t i = 0; i < finished.size(); ++i) {
#ifdef _WIN32
        if (finished[i]->thread_handle != NULL) {
            WaitForSingleObject(finished[i]->thread_handle, INFINITE);
            CloseHandle(finished[i]->thread_handle);
            finished[i]->thread_handle = NULL;
        }
#else
        if (finished[i]->thread != NULL) {
            finished[i]->thread->join();
            delete finished[i]->thread;
            finished[i]->thread = NULL;
        }
#endif
    }
}

unsigned long ConnectionManager::active_count() const {
    BasicLockGuard lock(mutex_);
    return static_cast<unsigned long>(workers_.size());
}
