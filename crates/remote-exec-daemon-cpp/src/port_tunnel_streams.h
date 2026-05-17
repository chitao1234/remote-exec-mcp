#pragma once

#include <memory>
#include <utility>
#include <vector>

#include "basic_mutex.h"
#include "port_tunnel_common.h"

class PortTunnelService;

enum class PortTunnelBudgetKind {
    None,
    RetainedSession,
    RetainedListener,
    UdpBind,
    ActiveTcpStream,
};

class PortTunnelBudgetLease {
public:
    PortTunnelBudgetLease();
    ~PortTunnelBudgetLease();

    static PortTunnelBudgetLease adopt(const std::shared_ptr<PortTunnelService>& service,
                                       PortTunnelBudgetKind kind);

    PortTunnelBudgetLease(PortTunnelBudgetLease&& other);
    PortTunnelBudgetLease& operator=(PortTunnelBudgetLease&& other);

    void reset();
    bool valid() const;

private:
    PortTunnelBudgetLease(const PortTunnelBudgetLease&);
    PortTunnelBudgetLease& operator=(const PortTunnelBudgetLease&);

    std::weak_ptr<PortTunnelService> service_;
    PortTunnelBudgetKind kind_;
};

struct TunnelTcpStream {
    TunnelTcpStream(SOCKET socket_value,
                    PortTunnelBudgetLease active_stream_budget_value)
        : socket(socket_value), active_stream_budget(std::move(active_stream_budget_value)), closed(false),
          writer_closed(false), writer_shutdown_requested(false) {}

    UniqueSocket socket;
    PortTunnelBudgetLease active_stream_budget;
    BasicMutex mutex;
    BasicCondVar writer_cond;
    std::vector<std::vector<unsigned char>> write_queue;
    bool closed;
    bool writer_closed;
    bool writer_shutdown_requested;
};

struct TunnelUdpSocket {
    TunnelUdpSocket(SOCKET socket_value, PortTunnelBudgetLease udp_bind_budget_value)
        : socket(socket_value), udp_bind_budget(std::move(udp_bind_budget_value)), closed(false) {}

    UniqueSocket socket;
    PortTunnelBudgetLease udp_bind_budget;
    BasicMutex mutex;
    bool closed;
};

struct RetainedTcpListener {
    RetainedTcpListener(uint32_t stream_id_value,
                        SOCKET listener_socket,
                        PortTunnelBudgetLease retained_listener_budget_value)
        : stream_id(stream_id_value), listener(listener_socket),
          retained_listener_budget(std::move(retained_listener_budget_value)), closed(false) {}

    uint32_t stream_id;
    UniqueSocket listener;
    PortTunnelBudgetLease retained_listener_budget;
    BasicMutex mutex;
    bool closed;
};

void mark_tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream);
void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value);
bool close_udp_socket_locked(TunnelUdpSocket* socket_value);
bool tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream);
bool udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value);
bool retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener);
void mark_retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener);
bool close_retained_listener_locked(RetainedTcpListener* listener);

class ConnectionLocalStreams {
public:
    ConnectionLocalStreams() {}

    void insert_tcp(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& stream);
    std::shared_ptr<TunnelTcpStream> get_tcp(uint32_t stream_id);
    std::shared_ptr<TunnelTcpStream> remove_tcp(uint32_t stream_id);
    void insert_udp(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value);
    std::shared_ptr<TunnelUdpSocket> get_udp(uint32_t stream_id);
    std::shared_ptr<TunnelUdpSocket> remove_udp(uint32_t stream_id);
    void drain(std::vector<std::shared_ptr<TunnelTcpStream>>* tcp_streams,
               std::vector<std::shared_ptr<TunnelUdpSocket>>* udp_sockets);

private:
    ConnectionLocalStreams(const ConnectionLocalStreams&);
    ConnectionLocalStreams& operator=(const ConnectionLocalStreams&);

    BasicMutex mutex_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>> tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>> udp_sockets_;
};
