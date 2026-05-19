#include "port_tunnel_spawn.h"

#include <atomic>

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
                           PortTunnelWorkerLease worker_lease,
                           const std::shared_ptr<TcpReadStartGate>& start_gate) {
#ifdef REMOTE_EXEC_CPP_TESTING
    if (consume_forced_tcp_read_thread_failure()) {
        if (!worker_lease.valid() && !service->try_acquire_worker(&worker_lease)) {
            return false;
        }
        return false;
    }
#endif
    return service->spawn_tracked_worker(
        "spawn tcp read thread",
        std::move(worker_lease),
        [tunnel, stream_id, stream, start_gate]() {
            if (start_gate.get() != nullptr) {
                start_gate->wait();
            }
            tunnel->tcp_read_loop(stream_id, stream);
        });
}

bool spawn_tcp_write_thread(const std::shared_ptr<PortTunnelService>& service,
                            const std::shared_ptr<PortTunnelConnection>& tunnel,
                            uint32_t stream_id,
                            const std::shared_ptr<TunnelTcpStream>& stream,
                            PortTunnelWorkerLease worker_lease) {
    return service->spawn_tracked_worker(
        "spawn tcp write thread", std::move(worker_lease), [tunnel, stream_id, stream]() {
            tunnel->tcp_write_loop(stream_id, stream);
        });
}

bool spawn_udp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelUdpSocket>& socket_value,
                           PortTunnelWorkerLease worker_lease) {
    return service->spawn_tracked_worker(
        "spawn udp read thread", std::move(worker_lease), [tunnel, stream_id, socket_value]() {
            tunnel->udp_read_loop_connection_local(stream_id, socket_value);
        });
}
