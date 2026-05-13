#pragma once

#include <atomic>
#include <memory>
#include <vector>

#include "port_tunnel_session_state.h"

class PortTunnelService;
class PortTunnelSender;

class PortTunnelConnection : public std::enable_shared_from_this<PortTunnelConnection> {
public:
    PortTunnelConnection(SOCKET client, const std::shared_ptr<PortTunnelService>& service);

    void run();
    void tcp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void tcp_write_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void udp_read_loop_connection_local(uint32_t stream_id, std::shared_ptr<TunnelUdpSocket> socket_value);
    void send_error(uint32_t stream_id, const std::string& code, const std::string& message);
    void send_terminal_error(uint32_t stream_id, const std::string& code, const std::string& message);
    void send_forward_drop(uint32_t stream_id,
                           const std::string& kind,
                           const std::string& reason,
                           const std::string& message);
    bool owns_session(const std::shared_ptr<PortTunnelSession>& session);
    bool accept_session_tcp_stream(const std::shared_ptr<PortTunnelSession>& session,
                                   uint32_t listener_stream_id,
                                   UniqueSocket accepted_socket,
                                   const std::string& peer);
    bool emit_session_udp_datagram(const std::shared_ptr<PortTunnelSession>& session,
                                   uint32_t stream_id,
                                   const std::string& peer,
                                   const std::vector<unsigned char>& data);

private:
    PortTunnelConnection(const PortTunnelConnection&) = delete;
    PortTunnelConnection& operator=(const PortTunnelConnection&) = delete;

    bool read_exact(unsigned char* data, std::size_t size);
    bool read_preface();
    bool read_frame(PortTunnelFrame* frame);
    void send_frame(const PortTunnelFrame& frame);
    bool send_data_frame_or_limit_error(const PortTunnelFrame& frame);
    bool send_data_frame_or_drop_on_limit(const PortTunnelFrame& frame);
    bool closed() const;
    void mark_closed();
    bool send_tcp_success_after_io_threads_started(const PortTunnelFrame& success,
                                                   ConnectionLocalStreams* local_streams,
                                                   uint32_t stream_id,
                                                   const std::shared_ptr<TunnelTcpStream>& stream,
                                                   bool worker_acquired);
    void drop_tcp_stream(ConnectionLocalStreams* local_streams,
                         uint32_t stream_id,
                         const std::shared_ptr<TunnelTcpStream>& fallback);
    void handle_frame(const PortTunnelFrame& frame);
    void tunnel_open(const PortTunnelFrame& frame);
    void tunnel_close(const PortTunnelFrame& frame);
    void tunnel_heartbeat(const PortTunnelFrame& frame);
    void tcp_listen(const PortTunnelFrame& frame);
    void tcp_connect(const PortTunnelFrame& frame);
    void tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data);
    void tcp_eof(uint32_t stream_id);
    void udp_bind(const PortTunnelFrame& frame);
    void udp_datagram(const PortTunnelFrame& frame);
    void close_stream(uint32_t stream_id);
    void send_worker_limit(uint32_t stream_id);
    std::uint64_t current_generation() const;
    void set_generation(std::uint64_t generation);
    void ensure_generation(std::uint64_t frame_generation) const;
    void close_current_session(PortTunnelCloseMode mode);
    void close_connection_local_state();
    std::shared_ptr<PortTunnelSession> current_session();
    std::shared_ptr<PortTunnelSessionAttachment> current_session_attachment();
    std::shared_ptr<PortTunnelSessionAttachment>
    session_attachment_for(const std::shared_ptr<PortTunnelSession>& session);
    std::shared_ptr<TunnelTcpStream> get_active_tcp_stream(uint32_t stream_id);
    std::shared_ptr<TunnelTcpStream> remove_active_tcp_stream(uint32_t stream_id);
    PortTunnelMode current_mode();
    void require_mode(PortTunnelMode mode, PortTunnelProtocol protocol, const std::string& message);
    bool session_mode_active();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    std::shared_ptr<PortTunnelSender> sender_;
    ConnectionLocalStreams connection_local_streams_;
    BasicMutex state_mutex_;
    std::atomic<std::uint64_t> generation_;
    std::shared_ptr<PortTunnelSession> session_;
    PortTunnelMode mode_;
    PortTunnelProtocol protocol_;
};
