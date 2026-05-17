#pragma once

#include <map>
#include <memory>
#include <string>

#include "port_tunnel_streams.h"

class PortTunnelConnection;
class PortTunnelService;

enum class PortTunnelRetainedResourceKind { None, TcpListener, UdpBind };

struct PortTunnelSessionAttachment {
    explicit PortTunnelSessionAttachment(const std::shared_ptr<PortTunnelConnection>& connection_value)
        : connection(connection_value) {}

    std::weak_ptr<PortTunnelConnection> connection;
    ConnectionLocalStreams local_streams;
};

struct PortTunnelRetainedResource {
    PortTunnelRetainedResource() : kind(PortTunnelRetainedResourceKind::None), stream_id(0U) {}

    PortTunnelRetainedResourceKind kind;
    uint32_t stream_id;
    std::shared_ptr<RetainedTcpListener> tcp_listener;
    std::shared_ptr<TunnelUdpSocket> udp_bind;
};

enum class SessionRetainedInstallResult { Installed, Conflict, Unavailable };
enum class PortTunnelSessionResumeResult { Ready, Unknown, AlreadyAttached, Expired };

struct PortTunnelSessionTeardown {
    PortTunnelSessionTeardown() : transitioned(false) {}

    bool transitioned;
    std::shared_ptr<PortTunnelSessionAttachment> attachment;
    std::shared_ptr<RetainedTcpListener> retained_listener;
    std::shared_ptr<TunnelUdpSocket> udp_bind;
};

struct PortTunnelSession {
    PortTunnelSession(const std::string& session_id_value,
                      const std::shared_ptr<PortTunnelService>& service_value,
                      PortTunnelBudgetLease retained_budget)
        : session_id(session_id_value), service(service_value), closed(false), expired(false), resume_deadline_ms(0ULL),
          generation(0ULL), retained_session_budget(std::move(retained_budget)), next_daemon_stream_id(2U) {}

    std::shared_ptr<PortTunnelSessionAttachment> attach(const std::shared_ptr<PortTunnelConnection>& connection);
    std::shared_ptr<PortTunnelSessionAttachment> detach_until(std::uint64_t deadline_ms, bool* detached);
    PortTunnelSessionTeardown close_terminal(bool mark_expired);
    PortTunnelSessionTeardown expire_if_due(std::uint64_t now_ms);
    bool detached_deadline(std::uint64_t* deadline_ms);
    PortTunnelSessionResumeResult prepare_resume(std::uint64_t generation_value, std::uint64_t now_ms);
    void set_generation(std::uint64_t generation_value);
    std::shared_ptr<PortTunnelSessionAttachment> current_attachment();
    bool insert_tcp_stream_if_attached(const std::shared_ptr<PortTunnelSessionAttachment>& expected_attachment,
                                       const std::shared_ptr<TunnelTcpStream>& stream,
                                       std::uint32_t* stream_id);
    SessionRetainedInstallResult install_tcp_listener(uint32_t stream_id,
                                                      const std::shared_ptr<RetainedTcpListener>& listener);
    SessionRetainedInstallResult install_udp_bind(uint32_t stream_id,
                                                  const std::shared_ptr<TunnelUdpSocket>& socket_value);
    std::shared_ptr<TunnelUdpSocket> udp_bind_for(uint32_t stream_id);
    PortTunnelRetainedResource remove_retained_resource(uint32_t stream_id, bool* removed);
    std::shared_ptr<PortTunnelSessionAttachment> wait_for_attachment(unsigned long wait_ms);
    bool is_unavailable();

    std::string session_id;
    std::weak_ptr<PortTunnelService> service;
    BasicMutex mutex;
    BasicCondVar state_changed;
    bool closed;
    bool expired;
    std::uint64_t resume_deadline_ms;
    std::uint64_t generation;
    PortTunnelBudgetLease retained_session_budget;
    std::shared_ptr<PortTunnelSessionAttachment> attachment;
    PortTunnelRetainedResource retained_resource;
    std::uint32_t next_daemon_stream_id;
};

bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session);
