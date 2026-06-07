#pragma once

// Lock ordering for the port tunnel subsystem.
//
// Nested acquisitions (must always be acquired in this order):
//   PortTunnelService::expiry_mutex_
//     -> PortTunnelSession::mutex          (expiry_scheduler_loop reads session state)
//       -> ConnectionLocalStreams::mutex_   (insert_tcp_stream_if_attached)
//
// Independent leaf locks (never held while acquiring another mutex, never
// acquired while another mutex is held):
//   PortTunnelService::mutex_        — released before calling session/worker methods
//   PortTunnelConnection::state_mutex_
//   PortTunnelSender::writer_mutex_
//   WorkerGroup::mutex
//   RetainedTcpListener::mutex
//   TunnelUdpSocket::mutex
//   TunnelTcpStream::mutex
//   TcpReadStartGate::mutex_
//
// Key invariants:
//   - Service::mutex_ is never held when calling into session methods.
//     close_session() erases from the map under mutex_, releases it, then
//     calls session->close_terminal().
//   - expiry_scheduler_loop releases expiry_mutex_ before calling
//     expire_session_if_needed() which may acquire Service::mutex_.
//   - Budget counters use atomics, not mutexes.

#include <functional>
#include <map>
#include <memory>
#include <thread>
#include <vector>

#include "core/config.h"
#include "port_tunnel_session_state.h"

class PortTunnelConnection;

class PortTunnelWorkerLease {
public:
    PortTunnelWorkerLease();
    explicit PortTunnelWorkerLease(const std::shared_ptr<PortTunnelBudgetState>& budget_state);
    PortTunnelWorkerLease(PortTunnelWorkerLease&& other);
    PortTunnelWorkerLease& operator=(PortTunnelWorkerLease&& other);
    ~PortTunnelWorkerLease();

    void reset();
    bool valid() const;

private:
    PortTunnelWorkerLease(const PortTunnelWorkerLease&);
    PortTunnelWorkerLease& operator=(const PortTunnelWorkerLease&);

    std::shared_ptr<PortTunnelBudgetState> budget_state_;
};

class PortTunnelService : public std::enable_shared_from_this<PortTunnelService> {
public:
    explicit PortTunnelService(const PortForwardLimitConfig& limits);
    ~PortTunnelService();

    // Owns retained tunnel sessions, budget state, retained worker records, and
    // the expiry scheduler. shutdown() is the authoritative terminal path:
    // sessions are transitioned to terminal state, resources are closed outside
    // service locks, and worker/scheduler threads are consumed by their owners.
    void shutdown();
    std::shared_ptr<PortTunnelSession> create_session();
    std::shared_ptr<PortTunnelSession> find_session(const std::string& session_id);
    bool attach_new_session(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<PortTunnelConnection>& connection,
        std::uint64_t generation
    );
    PortTunnelSessionResumeResult attach_resumed_session(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<PortTunnelConnection>& connection,
        std::uint64_t generation,
        std::uint64_t now_ms
    );
    void detach_session(const std::shared_ptr<PortTunnelSession>& session);
    void close_session(const std::shared_ptr<PortTunnelSession>& session);
    SessionRetainedInstallResult install_session_tcp_listener(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::shared_ptr<RetainedTcpListener>& listener
    );
    SessionRetainedInstallResult install_session_udp_bind(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::shared_ptr<TunnelUdpSocket>& socket_value
    );
    std::shared_ptr<TunnelUdpSocket> session_udp_bind(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id
    );
    bool close_session_retained_resource(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id
    );
    bool spawn_tcp_listener_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<RetainedTcpListener>& listener,
        PortTunnelWorkerLease worker_lease = PortTunnelWorkerLease()
    );
    bool spawn_udp_bind_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::shared_ptr<TunnelUdpSocket>& socket_value,
        PortTunnelWorkerLease worker_lease = PortTunnelWorkerLease()
    );
    bool spawn_tracked_worker(
        const char* operation,
        PortTunnelWorkerLease worker_lease,
        const std::function<void()>& work
    );
    bool try_acquire_worker();
    bool try_acquire_worker(PortTunnelWorkerLease* lease);
    unsigned long max_workers() const;
    const PortForwardLimitConfig& limits() const;
    bool try_acquire_retained_session(PortTunnelBudgetLease* lease);
    bool try_acquire_retained_session();
    bool try_acquire_retained_listener(PortTunnelBudgetLease* lease);
    bool try_acquire_retained_listener();
    bool try_acquire_udp_bind(PortTunnelBudgetLease* lease);
    bool try_acquire_udp_bind();
    bool try_acquire_active_tcp_stream(PortTunnelBudgetLease* lease);
    bool try_acquire_active_tcp_stream();

private:
    struct WorkerGroup;
    enum class LifecycleState { Running, Stopping, Stopped };

    PortTunnelService(const PortTunnelService&);
    PortTunnelService& operator=(const PortTunnelService&);

    bool begin_shutdown();
    void finish_shutdown();
    bool is_running();
    bool is_running_locked() const;
    void join_all_workers();
    void close_all_sessions_for_shutdown();
    bool schedule_session_expiry(const std::shared_ptr<PortTunnelSession>& session);
    bool ensure_expiry_scheduler_started_locked();
    void stop_expiry_scheduler();
    void expiry_scheduler_loop();
    void expire_session_if_needed(const std::shared_ptr<PortTunnelSession>& session);
    std::shared_ptr<PortTunnelSessionAttachment> wait_for_attachment(
        const std::shared_ptr<PortTunnelSession>& session
    );
    void tcp_accept_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<RetainedTcpListener>& listener
    );
    void udp_read_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::shared_ptr<TunnelUdpSocket>& socket_value
    );

    BasicMutex mutex_;
    LifecycleState lifecycle_state_;
    std::shared_ptr<PortTunnelBudgetState> budget_state_;
    std::unique_ptr<WorkerGroup> worker_group_;
    PortForwardLimitConfig limits_;
    std::map<std::string, std::shared_ptr<PortTunnelSession>> sessions_;
    std::uint64_t next_session_sequence_;
    BasicMutex expiry_mutex_;
    BasicCondVar expiry_cond_;
    std::vector<std::weak_ptr<PortTunnelSession>> expiry_sessions_;
    bool expiry_shutdown_;
    bool expiry_thread_started_;
    std::unique_ptr<std::thread> expiry_thread_;
};
