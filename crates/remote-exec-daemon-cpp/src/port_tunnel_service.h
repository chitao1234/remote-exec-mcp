#pragma once

#include <atomic>
#include <functional>
#include <map>
#include <memory>
#include <thread>
#include <vector>

#include "config.h"
#include "port_tunnel_session_state.h"

class PortTunnelConnection;

class PortTunnelService : public std::enable_shared_from_this<PortTunnelService> {
public:
    explicit PortTunnelService(const PortForwardLimitConfig& limits);
    ~PortTunnelService();

    std::shared_ptr<PortTunnelSession> create_session();
    std::shared_ptr<PortTunnelSession> find_session(const std::string& session_id);
    void attach_session(const std::shared_ptr<PortTunnelSession>& session,
                        const std::shared_ptr<PortTunnelConnection>& connection);
    void detach_session(const std::shared_ptr<PortTunnelSession>& session);
    void close_session(const std::shared_ptr<PortTunnelSession>& session);
    SessionRetainedInstallResult install_session_tcp_listener(const std::shared_ptr<PortTunnelSession>& session,
                                                              uint32_t stream_id,
                                                              const std::shared_ptr<RetainedTcpListener>& listener);
    SessionRetainedInstallResult install_session_udp_bind(const std::shared_ptr<PortTunnelSession>& session,
                                                          uint32_t stream_id,
                                                          const std::shared_ptr<TunnelUdpSocket>& socket_value);
    std::shared_ptr<TunnelUdpSocket> session_udp_bind(const std::shared_ptr<PortTunnelSession>& session,
                                                      uint32_t stream_id);
    bool close_session_retained_resource(const std::shared_ptr<PortTunnelSession>& session, uint32_t stream_id);
    bool spawn_tcp_listener_loop(const std::shared_ptr<PortTunnelSession>& session,
                                 const std::shared_ptr<RetainedTcpListener>& listener,
                                 bool worker_acquired = false);
    bool spawn_udp_bind_loop(const std::shared_ptr<PortTunnelSession>& session,
                             uint32_t stream_id,
                             const std::shared_ptr<TunnelUdpSocket>& socket_value,
                             bool worker_acquired = false);
    bool spawn_tracked_worker(const char* operation,
                              bool worker_acquired,
                              const std::function<void()>& work);
    bool try_acquire_worker();
    void release_worker();
    unsigned long max_workers() const;
    const PortForwardLimitConfig& limits() const;
    bool try_acquire_retained_session(PortTunnelBudgetLease* lease);
    bool try_acquire_retained_session();
    void release_retained_session();
    bool try_acquire_retained_listener(PortTunnelBudgetLease* lease);
    bool try_acquire_retained_listener();
    void release_retained_listener();
    bool try_acquire_udp_bind(PortTunnelBudgetLease* lease);
    bool try_acquire_udp_bind();
    void release_udp_bind();
    bool try_acquire_active_tcp_stream(PortTunnelBudgetLease* lease);
    bool try_acquire_active_tcp_stream();
    void release_active_tcp_stream();

private:
    struct TrackedWorkerThread;

    PortTunnelService(const PortTunnelService&);
    PortTunnelService& operator=(const PortTunnelService&);

    void mark_worker_finished(const std::shared_ptr<TrackedWorkerThread>& worker);
    void collect_finished_workers(std::vector<std::shared_ptr<TrackedWorkerThread>>* finished_workers);
    void join_workers(const std::vector<std::shared_ptr<TrackedWorkerThread>>& workers);
    void join_all_workers();
    void close_all_sessions_for_shutdown();
    bool schedule_session_expiry(const std::shared_ptr<PortTunnelSession>& session);
    bool ensure_expiry_scheduler_started_locked();
    void stop_expiry_scheduler();
    void expiry_scheduler_loop();
    void expire_session_if_needed(const std::shared_ptr<PortTunnelSession>& session);
    std::shared_ptr<PortTunnelSessionAttachment> wait_for_attachment(const std::shared_ptr<PortTunnelSession>& session);
    void tcp_accept_loop(const std::shared_ptr<PortTunnelSession>& session,
                         const std::shared_ptr<RetainedTcpListener>& listener);
    void udp_read_loop(const std::shared_ptr<PortTunnelSession>& session,
                       uint32_t stream_id,
                       const std::shared_ptr<TunnelUdpSocket>& socket_value);

    BasicMutex mutex_;
    std::atomic<unsigned long> active_workers_;
    std::atomic<unsigned long> retained_sessions_;
    std::atomic<unsigned long> retained_listeners_;
    std::atomic<unsigned long> udp_binds_;
    std::atomic<unsigned long> active_tcp_streams_;
    BasicMutex worker_threads_mutex_;
    std::vector<std::shared_ptr<TrackedWorkerThread>> worker_threads_;
    PortForwardLimitConfig limits_;
    std::map<std::string, std::shared_ptr<PortTunnelSession>> sessions_;
    std::uint64_t next_session_sequence_;
    BasicMutex expiry_mutex_;
    BasicCondVar expiry_cond_;
    std::vector<std::weak_ptr<PortTunnelSession>> expiry_sessions_;
    bool expiry_shutdown_;
    bool expiry_thread_started_;
#ifdef _WIN32
    HANDLE expiry_thread_;
    static unsigned __stdcall expiry_thread_entry(void* raw_context);
#else
    std::unique_ptr<std::thread> expiry_thread_;
#endif
};

class PortTunnelWorkerLease {
public:
    explicit PortTunnelWorkerLease(PortTunnelService* service);
    ~PortTunnelWorkerLease();

private:
    PortTunnelWorkerLease(const PortTunnelWorkerLease&);
    PortTunnelWorkerLease& operator=(const PortTunnelWorkerLease&);

    PortTunnelService* service_;
};
