#pragma once

#include <atomic>
#include <map>
#include <memory>
#include <utility>
#include <vector>

#include "platform/basic_mutex.h"
#include "platform/wakeup_pipe.h"
#include "port_tunnel_common.h"

class PortTunnelService;

struct PortTunnelBudgetState {
    PortTunnelBudgetState()
        : active_workers(0UL), retained_sessions(0UL), retained_listeners(0UL), udp_binds(0UL),
          active_tcp_streams(0UL) {}

    std::atomic<unsigned long> active_workers;
    std::atomic<unsigned long> retained_sessions;
    std::atomic<unsigned long> retained_listeners;
    std::atomic<unsigned long> udp_binds;
    std::atomic<unsigned long> active_tcp_streams;
};

enum class PortTunnelBudgetKind {
    None,
    RetainedSession,
    RetainedListener,
    UdpBind,
    ActiveTcpStream,
};

enum class PortTunnelResourceState { Open, Closing, Closed };

const char* port_tunnel_resource_state_name(PortTunnelResourceState state);

// Shared resource-state plumbing for tunnel resources: each resource owns a
// state (open/closing/closed) guarded by its mutex and exposes the same
// is_closed / is_closing_or_closed_locked / resource_state_snapshot surface.
struct PortTunnelResourceStateOwner {
    PortTunnelResourceStateOwner() : resource_state(PortTunnelResourceState::Open) {}

    bool is_closed() {
        BasicLockGuard lock(mutex);
        return is_closing_or_closed_locked();
    }

    bool is_closing_or_closed_locked() const {
        return resource_state != PortTunnelResourceState::Open;
    }

    PortTunnelResourceState resource_state_snapshot() {
        BasicLockGuard lock(mutex);
        return resource_state;
    }

    BasicMutex mutex;
    PortTunnelResourceState resource_state;
};

class PortTunnelBudgetLease {
public:
    PortTunnelBudgetLease();
    ~PortTunnelBudgetLease();

    static PortTunnelBudgetLease adopt(
        const std::shared_ptr<PortTunnelBudgetState>& budget_state,
        PortTunnelBudgetKind kind
    );

    PortTunnelBudgetLease(PortTunnelBudgetLease&& other);
    PortTunnelBudgetLease& operator=(PortTunnelBudgetLease&& other);

    void reset();
    bool valid() const;

private:
    PortTunnelBudgetLease(const PortTunnelBudgetLease&);
    PortTunnelBudgetLease& operator=(const PortTunnelBudgetLease&);

    std::shared_ptr<PortTunnelBudgetState> budget_state_;
    PortTunnelBudgetKind kind_;
};

struct TunnelTcpStream : PortTunnelResourceStateOwner {
    TunnelTcpStream(SOCKET socket_value, PortTunnelBudgetLease active_stream_budget_value)
        : socket(socket_value), active_stream_budget(std::move(active_stream_budget_value)),
          writer_closed(false), writer_shutdown_requested(false) {}

    void close();

    UniqueSocket socket;
    PortTunnelBudgetLease active_stream_budget;
    BasicCondVar writer_cond;
    std::vector<std::vector<unsigned char>> write_queue;
    bool writer_closed;
    bool writer_shutdown_requested;
};

struct TunnelUdpSocket : PortTunnelResourceStateOwner {
    TunnelUdpSocket(SOCKET socket_value, PortTunnelBudgetLease udp_bind_budget_value)
        : socket(socket_value), udp_bind_budget(std::move(udp_bind_budget_value)) {}

    void close();

    UniqueSocket socket;
    PortTunnelBudgetLease udp_bind_budget;
    WakeupPipe wakeup;
};

struct RetainedTcpListener : PortTunnelResourceStateOwner {
    RetainedTcpListener(
        uint32_t stream_id_value,
        SOCKET listener_socket,
        PortTunnelBudgetLease retained_listener_budget_value
    )
        : stream_id(stream_id_value), listener(listener_socket),
          retained_listener_budget(std::move(retained_listener_budget_value)) {}

    void close();

    uint32_t stream_id;
    UniqueSocket listener;
    PortTunnelBudgetLease retained_listener_budget;
    WakeupPipe wakeup;
};

class ConnectionLocalStreams {
public:
    ConnectionLocalStreams() {}

    void insert_tcp(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& stream);
    std::shared_ptr<TunnelTcpStream> get_tcp(uint32_t stream_id);
    std::shared_ptr<TunnelTcpStream> remove_tcp(uint32_t stream_id);
    void insert_udp(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value);
    std::shared_ptr<TunnelUdpSocket> get_udp(uint32_t stream_id);
    std::shared_ptr<TunnelUdpSocket> remove_udp(uint32_t stream_id);
    void drain(
        std::vector<std::shared_ptr<TunnelTcpStream>>* tcp_streams,
        std::vector<std::shared_ptr<TunnelUdpSocket>>* udp_sockets
    );

private:
    ConnectionLocalStreams(const ConnectionLocalStreams&);
    ConnectionLocalStreams& operator=(const ConnectionLocalStreams&);

    BasicMutex mutex_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>> tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>> udp_sockets_;
};
