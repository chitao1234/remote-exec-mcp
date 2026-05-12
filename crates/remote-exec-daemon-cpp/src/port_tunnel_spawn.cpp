#include "port_tunnel_spawn.h"

#include <atomic>
#include <functional>

#include "port_tunnel_connection.h"
#include "port_tunnel_service.h"

#ifdef REMOTE_EXEC_CPP_TESTING
static std::atomic<unsigned long> g_forced_tcp_read_thread_failures(0UL);

void set_forced_tcp_read_thread_failures(unsigned long count) {
    g_forced_tcp_read_thread_failures.store(count);
}

static bool consume_forced_tcp_read_thread_failure() {
    unsigned long current = g_forced_tcp_read_thread_failures.load();
    while (current > 0UL) {
        if (g_forced_tcp_read_thread_failures.compare_exchange_weak(current, current - 1UL)) {
            return true;
        }
    }
    return false;
}
#endif

namespace {

#ifdef _WIN32
struct WorkerThreadContext {
    std::shared_ptr<PortTunnelService> service;
    std::function<void()> work;
};

unsigned __stdcall worker_thread_entry(void* raw_context) {
    std::unique_ptr<WorkerThreadContext> context(static_cast<WorkerThreadContext*>(raw_context));
    PortTunnelWorkerLease lease(context->service);
    context->work();
    return 0;
}
#endif

bool spawn_worker_thread(const char* operation,
                         const std::shared_ptr<PortTunnelService>& service,
                         bool worker_acquired,
                         const std::function<void()>& work) {
    if (!worker_acquired && !service->try_acquire_worker()) {
        return false;
    }
#ifdef _WIN32
    std::unique_ptr<WorkerThreadContext> context(new WorkerThreadContext());
    context->service = service;
    context->work = work;
    HANDLE handle = begin_win32_thread(worker_thread_entry, context.get());
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
        return true;
    }
    service->release_worker();
    return false;
#else
    try {
        std::thread([service, work]() {
            PortTunnelWorkerLease lease(service);
            work();
        }).detach();
    } catch (const std::exception& ex) {
        log_tunnel_exception(operation, ex);
        service->release_worker();
        return false;
    } catch (...) {
        log_unknown_tunnel_exception(operation);
        service->release_worker();
        return false;
    }
    return true;
#endif
}

} // namespace

TcpReadStartGate::TcpReadStartGate() : released_(false) {
}

void TcpReadStartGate::release() {
    BasicLockGuard lock(mutex_);
    released_ = true;
    cond_.broadcast();
}

void TcpReadStartGate::wait() {
    BasicLockGuard lock(mutex_);
    while (!released_) {
        cond_.wait(mutex_);
    }
}

bool spawn_tcp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelTcpStream>& stream,
                           bool worker_acquired,
                           const std::shared_ptr<TcpReadStartGate>& start_gate) {
#ifdef REMOTE_EXEC_CPP_TESTING
    if (consume_forced_tcp_read_thread_failure()) {
        if (!worker_acquired && !service->try_acquire_worker()) {
            return false;
        }
        service->release_worker();
        return false;
    }
#endif
    return spawn_worker_thread("spawn tcp read thread", service, worker_acquired, [tunnel, stream_id, stream, start_gate]() {
        if (start_gate.get() != NULL) {
            start_gate->wait();
        }
        tunnel->tcp_read_loop(stream_id, stream);
    });
}

bool spawn_tcp_write_thread(const std::shared_ptr<PortTunnelService>& service,
                            const std::shared_ptr<PortTunnelConnection>& tunnel,
                            uint32_t stream_id,
                            const std::shared_ptr<TunnelTcpStream>& stream,
                            bool worker_acquired) {
    return spawn_worker_thread("spawn tcp write thread", service, worker_acquired, [tunnel, stream_id, stream]() {
        tunnel->tcp_write_loop(stream_id, stream);
    });
}

bool spawn_udp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelUdpSocket>& socket_value,
                           bool worker_acquired) {
    return spawn_worker_thread("spawn udp read thread", service, worker_acquired, [tunnel, stream_id, socket_value]() {
        tunnel->udp_read_loop_transport_owned(stream_id, socket_value);
    });
}
