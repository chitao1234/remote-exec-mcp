#include "port_tunnel_workers.h"

#include <exception>
#include <utility>

#include "runtime/daemon_thread.h"

class PortTunnelWorkerStartGate {
public:
    PortTunnelWorkerStartGate() : released_(false), cancelled_(false) {}

    void release() {
        BasicLockGuard lock(mutex_);
        released_ = true;
        cond_.broadcast();
    }

    void cancel() {
        BasicLockGuard lock(mutex_);
        cancelled_ = true;
        released_ = true;
        cond_.broadcast();
    }

    bool wait() {
        BasicLockGuard lock(mutex_);
        while (!released_) {
            cond_.wait(mutex_);
        }
        return !cancelled_;
    }

private:
    BasicMutex mutex_;
    BasicCondVar cond_;
    bool released_;
    bool cancelled_;
};

PortTunnelService::WorkerGroup::WorkerGroup() : shutting_down(false) {
}

PortTunnelService::WorkerGroup::Thread::Thread() : finished(false)
                                                 ,
                                                 thread()
{
}

bool PortTunnelService::spawn_tracked_worker(const char* operation,
                                             PortTunnelWorkerLease worker_lease,
                                             const std::function<void()>& work) {
    return worker_group_->spawn(shared_from_this(), operation, std::move(worker_lease), work);
}

bool PortTunnelService::WorkerGroup::spawn(const std::shared_ptr<PortTunnelService>& service,
                                           const char* operation,
                                           PortTunnelWorkerLease worker_lease,
                                           const std::function<void()>& work) {
    if (!service->is_running()) {
        return false;
    }
    if (!worker_lease.valid() && !service->try_acquire_worker(&worker_lease)) {
        return false;
    }
    std::shared_ptr<PortTunnelWorkerLease> worker_lease_holder(new PortTunnelWorkerLease(std::move(worker_lease)));
    std::shared_ptr<PortTunnelWorkerStartGate> start_gate(new PortTunnelWorkerStartGate());

    std::vector<std::shared_ptr<Thread>> finished_workers;
    collect_finished(&finished_workers);

    std::shared_ptr<Thread> worker(new Thread());
    bool worker_published = false;

    try {
        BasicLockGuard lock(mutex);
        if (!shutting_down) {
            worker->thread.reset(new std::thread([service, worker, worker_lease_holder, start_gate, work, operation]() {
                if (!start_gate->wait()) {
                    worker->finished.store(true);
                    return;
                }
                log_message(LOG_DEBUG,
                            "port_tunnel",
                            LogMessageBuilder("worker start").quoted_field("operation", operation).str());
                try {
                    work();
                } catch (const std::exception& ex) {
                    log_tunnel_exception(operation, ex);
                } catch (...) {
                    log_unknown_tunnel_exception(operation);
                }
                log_message(LOG_DEBUG,
                            "port_tunnel",
                            LogMessageBuilder("worker finish").quoted_field("operation", operation).str());
                worker->finished.store(true);
            }));
            threads.push_back(worker);
            worker_published = true;
        }
    } catch (const std::exception& ex) {
        if (worker->thread.get() != nullptr) {
            start_gate->cancel();
            consume_daemon_thread(&worker->thread);
        }
        join_workers(finished_workers);
        log_tunnel_exception(operation, ex);
        return false;
    } catch (...) {
        if (worker->thread.get() != nullptr) {
            start_gate->cancel();
            consume_daemon_thread(&worker->thread);
        }
        join_workers(finished_workers);
        log_unknown_tunnel_exception(operation);
        return false;
    }

    if (!worker_published) {
        join_workers(finished_workers);
        return false;
    }

    worker_lease_holder.reset();
    start_gate->release();
    join_workers(finished_workers);
    return true;
}

void PortTunnelService::WorkerGroup::begin_shutdown() {
    BasicLockGuard lock(mutex);
    shutting_down = true;
}

void PortTunnelService::WorkerGroup::collect_finished(std::vector<std::shared_ptr<Thread>>* finished_workers) {
    BasicLockGuard lock(mutex);
    for (std::vector<std::shared_ptr<Thread>>::iterator it = threads.begin(); it != threads.end();) {
        if (!(*it)->finished.load()) {
            ++it;
            continue;
        }
        finished_workers->push_back(*it);
        it = threads.erase(it);
    }
}

void PortTunnelService::WorkerGroup::join_workers(const std::vector<std::shared_ptr<Thread>>& workers) {
    for (std::size_t i = 0; i < workers.size(); ++i) {
        consume_daemon_thread(&workers[i]->thread);
    }
}

void PortTunnelService::join_all_workers() {
    worker_group_->join_all();
}

void PortTunnelService::WorkerGroup::join_all() {
    std::vector<std::shared_ptr<Thread>> workers;
    {
        BasicLockGuard lock(mutex);
        workers.swap(threads);
    }
    join_workers(workers);
}
