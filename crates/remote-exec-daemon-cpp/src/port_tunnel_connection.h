#pragma once

#include <atomic>
#include <deque>
#include <memory>
#include <vector>

#include "port_tunnel_streams.h"

class PortTunnelService;
class PortTunnelConnection;

class TcpReadStartGate {
public:
    TcpReadStartGate();

    void release();
    void wait();

private:
    TcpReadStartGate(const TcpReadStartGate&);
    TcpReadStartGate& operator=(const TcpReadStartGate&);

    BasicMutex mutex_;
    BasicCondVar cond_;
    bool released_;
};

bool spawn_tcp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelTcpStream>& stream,
                           bool worker_acquired = false,
                           const std::shared_ptr<TcpReadStartGate>& start_gate = std::shared_ptr<TcpReadStartGate>());
bool spawn_tcp_write_thread(const std::shared_ptr<PortTunnelService>& service,
                            const std::shared_ptr<PortTunnelConnection>& tunnel,
                            uint32_t stream_id,
                            const std::shared_ptr<TunnelTcpStream>& stream,
                            bool worker_acquired = false);
bool spawn_udp_read_thread(const std::shared_ptr<PortTunnelService>& service,
                           const std::shared_ptr<PortTunnelConnection>& tunnel,
                           uint32_t stream_id,
                           const std::shared_ptr<TunnelUdpSocket>& socket_value,
                           bool worker_acquired = false);

class PortTunnelSender : public std::enable_shared_from_this<PortTunnelSender> {
public:
    PortTunnelSender(SOCKET client, const std::shared_ptr<PortTunnelService>& service);

    bool closed() const;
    void mark_closed();
    void send_frame(const PortTunnelFrame& frame);
    bool send_data_frame_or_limit_error(PortTunnelConnection& connection, const PortTunnelFrame& frame);
    bool send_data_frame_or_drop_on_limit(PortTunnelConnection& connection, const PortTunnelFrame& frame);

private:
    PortTunnelSender(const PortTunnelSender&);
    PortTunnelSender& operator=(const PortTunnelSender&);

    struct QueuedFrame {
        QueuedFrame() : charge_value(0UL) {}
        QueuedFrame(std::vector<unsigned char> bytes_value, unsigned long charge)
            : bytes(std::move(bytes_value)), charge_value(charge) {}

        std::vector<unsigned char> bytes;
        unsigned long charge_value;
    };

    void writer_loop();
    bool ensure_writer_started_locked();
    bool enqueue_encoded_frame(std::vector<unsigned char> bytes, unsigned long charge_value);
    bool try_reserve_data_frame(const PortTunnelFrame& frame, unsigned long* charge_value);
    void release_data_frame_reservation(unsigned long charge_value);
    void release_queued_frame_reservation(unsigned long charge_value);
    void drain_queued_frame_reservations_locked();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    BasicMutex writer_mutex_;
    BasicCondVar writer_cond_;
    std::deque<QueuedFrame> writer_queue_;
    bool writer_started_;
    bool writer_shutdown_;
    bool writer_finished_;
    std::atomic<bool> closed_;
    std::atomic<unsigned long> queued_bytes_;
};

class PortTunnelConnection : public std::enable_shared_from_this<PortTunnelConnection> {
public:
    PortTunnelConnection(SOCKET client, const std::shared_ptr<PortTunnelService>& service);

    void run();
    void tcp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void tcp_write_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void udp_read_loop_transport_owned(uint32_t stream_id, std::shared_ptr<TunnelUdpSocket> socket_value);
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
                                                   uint32_t stream_id,
                                                   const std::shared_ptr<TunnelTcpStream>& stream,
                                                   bool worker_acquired);
    void drop_tcp_stream(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& fallback);
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
    void close_transport_owned_state();
    std::shared_ptr<PortTunnelSession> current_session();
    PortTunnelMode current_mode();
    void require_mode(PortTunnelMode mode, PortTunnelProtocol protocol, const std::string& message);
    bool session_mode_active();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    std::shared_ptr<PortTunnelSender> sender_;
    TransportOwnedStreams transport_streams_;
    BasicMutex state_mutex_;
    std::atomic<std::uint64_t> generation_;
    std::shared_ptr<PortTunnelSession> session_;
    PortTunnelMode mode_;
    PortTunnelProtocol protocol_;
};
