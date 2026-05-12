#pragma once

#include <map>
#include <memory>
#include <string>
#include <utility>
#include <vector>

#include "basic_mutex.h"
#include "port_tunnel_common.h"

class PortTunnelConnection;
class PortTunnelService;

struct TunnelTcpStream {
    TunnelTcpStream(SOCKET socket_value,
                    const std::shared_ptr<PortTunnelService>& service_value,
                    bool active_stream_budget)
        : socket(socket_value), service(service_value), closed(false),
          active_stream_budget_acquired(active_stream_budget), writer_closed(false), writer_shutdown_requested(false) {}

    UniqueSocket socket;
    std::weak_ptr<PortTunnelService> service;
    BasicMutex mutex;
    BasicCondVar writer_cond;
    std::vector<std::vector<unsigned char>> write_queue;
    bool closed;
    bool active_stream_budget_acquired;
    bool writer_closed;
    bool writer_shutdown_requested;
};

struct TunnelUdpSocket {
    TunnelUdpSocket(SOCKET socket_value, const std::shared_ptr<PortTunnelService>& service_value, bool udp_bind_budget)
        : socket(socket_value), service(service_value), closed(false), udp_bind_budget_acquired(udp_bind_budget) {}

    UniqueSocket socket;
    std::weak_ptr<PortTunnelService> service;
    BasicMutex mutex;
    bool closed;
    bool udp_bind_budget_acquired;
};

struct RetainedTcpListener {
    RetainedTcpListener(uint32_t stream_id_value,
                        SOCKET listener_socket,
                        const std::shared_ptr<PortTunnelService>& service_value,
                        bool retained_listener_budget)
        : stream_id(stream_id_value), listener(listener_socket), service(service_value), closed(false),
          retained_listener_budget_acquired(retained_listener_budget) {}

    uint32_t stream_id;
    UniqueSocket listener;
    std::weak_ptr<PortTunnelService> service;
    BasicMutex mutex;
    bool closed;
    bool retained_listener_budget_acquired;
};

struct PortTunnelSession {
    PortTunnelSession(const std::string& session_id_value,
                      const std::shared_ptr<PortTunnelService>& service_value,
                      bool retained_budget)
        : session_id(session_id_value), service(service_value), attached(false), closed(false), expired(false),
          resume_deadline_ms(0ULL), generation(0ULL), retained_session_budget_acquired(retained_budget),
          next_daemon_stream_id(2U) {}

    std::string session_id;
    std::weak_ptr<PortTunnelService> service;
    BasicMutex mutex;
    BasicCondVar state_changed;
    bool attached;
    bool closed;
    bool expired;
    std::uint64_t resume_deadline_ms;
    std::uint64_t generation;
    bool retained_session_budget_acquired;
    std::weak_ptr<PortTunnelConnection> connection;
    std::map<uint32_t, std::shared_ptr<RetainedTcpListener>> tcp_listeners;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>> udp_binds;
    std::uint32_t next_daemon_stream_id;
};

void mark_tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream);
void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value);
bool tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream);
bool udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value);
bool retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener);
bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session);
void mark_retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener);

class TransportOwnedStreams {
public:
    TransportOwnedStreams() {}

    void insert_tcp(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& stream);
    std::shared_ptr<TunnelTcpStream> get_tcp(uint32_t stream_id);
    std::shared_ptr<TunnelTcpStream> remove_tcp(uint32_t stream_id);
    void insert_udp(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value);
    std::shared_ptr<TunnelUdpSocket> get_udp(uint32_t stream_id);
    std::shared_ptr<TunnelUdpSocket> remove_udp(uint32_t stream_id);
    void drain(std::vector<std::shared_ptr<TunnelTcpStream>>* tcp_streams,
               std::vector<std::shared_ptr<TunnelUdpSocket>>* udp_sockets);

private:
    TransportOwnedStreams(const TransportOwnedStreams&);
    TransportOwnedStreams& operator=(const TransportOwnedStreams&);

    BasicMutex mutex_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream>> tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>> udp_sockets_;
};
