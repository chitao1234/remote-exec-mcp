#pragma once

#include <map>
#include <memory>
#include <string>

#include "port_tunnel_streams.h"

class PortTunnelConnection;
class PortTunnelService;

struct PortTunnelSessionAttachment {
    explicit PortTunnelSessionAttachment(const std::shared_ptr<PortTunnelConnection>& connection_value)
        : connection(connection_value) {}

    std::weak_ptr<PortTunnelConnection> connection;
    ConnectionLocalStreams local_streams;
};

struct PortTunnelSession {
    PortTunnelSession(const std::string& session_id_value,
                      const std::shared_ptr<PortTunnelService>& service_value,
                      bool retained_budget)
        : session_id(session_id_value), service(service_value), closed(false), expired(false), resume_deadline_ms(0ULL),
          generation(0ULL), retained_session_budget_acquired(retained_budget), next_daemon_stream_id(2U) {}

    std::string session_id;
    std::weak_ptr<PortTunnelService> service;
    BasicMutex mutex;
    BasicCondVar state_changed;
    bool closed;
    bool expired;
    std::uint64_t resume_deadline_ms;
    std::uint64_t generation;
    bool retained_session_budget_acquired;
    std::shared_ptr<PortTunnelSessionAttachment> attachment;
    std::map<uint32_t, std::shared_ptr<RetainedTcpListener>> tcp_listeners;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket>> udp_binds;
    std::uint32_t next_daemon_stream_id;
};

bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session);
