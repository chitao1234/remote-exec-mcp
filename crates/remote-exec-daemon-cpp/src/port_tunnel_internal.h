#pragma once

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <map>
#include <memory>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#else
#include <thread>
#endif

#include "basic_mutex.h"
#include "json.hpp"
#include "platform.h"
#include "port_forward_endpoint.h"
#include "port_forward_error.h"
#include "port_forward_socket_ops.h"
#include "port_tunnel.h"
#include "port_tunnel_frame.h"
#include "server_transport.h"
#include "text_utils.h"

using Json = nlohmann::json;

extern const std::size_t READ_BUF_SIZE;
extern const unsigned long RETAINED_SOCKET_POLL_TIMEOUT_MS;
extern const unsigned long RESUME_TIMEOUT_MS;

enum class PortTunnelCloseMode {
    RetryableDetach,
    TerminalFailure,
};

struct TunnelTcpStream {
    explicit TunnelTcpStream(SOCKET socket_value) : socket(socket_value), closed(false) {}

    UniqueSocket socket;
    BasicMutex mutex;
    bool closed;
};

struct TunnelUdpSocket {
    explicit TunnelUdpSocket(SOCKET socket_value) : socket(socket_value), closed(false) {}

    UniqueSocket socket;
    BasicMutex mutex;
    bool closed;
};

struct RetainedTcpListener {
    RetainedTcpListener(uint32_t stream_id_value, SOCKET listener_socket)
        : stream_id(stream_id_value), listener(listener_socket), closed(false) {}

    uint32_t stream_id;
    UniqueSocket listener;
    BasicMutex mutex;
    bool closed;
};

class PortTunnelConnection;
class PortTunnelService;

struct PortTunnelSession {
    explicit PortTunnelSession(const std::string& session_id_value)
        : session_id(session_id_value),
          attached(false),
          closed(false),
          expired(false),
          resume_deadline_ms(0ULL),
          generation(0ULL),
          next_daemon_stream_id(2U) {}

    std::string session_id;
    BasicMutex mutex;
    BasicCondVar state_changed;
    bool attached;
    bool closed;
    bool expired;
    std::uint64_t resume_deadline_ms;
    std::uint64_t generation;
    std::weak_ptr<PortTunnelConnection> connection;
    std::map<uint32_t, std::shared_ptr<RetainedTcpListener> > tcp_listeners;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_binds;
    std::uint32_t next_daemon_stream_id;
};

std::string header_token_lower(const HttpRequest& request, const std::string& name);
bool connection_header_has_upgrade(const HttpRequest& request);
std::string frame_meta_string(const PortTunnelFrame& frame, const std::string& key);
PortTunnelFrame make_empty_frame(PortTunnelFrameType type, uint32_t stream_id);
void mark_tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream);
void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value);
bool tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream);
bool udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value);
bool retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener);
bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session);
int wait_socket_readable(SOCKET socket, unsigned long timeout_ms);
void mark_retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener);
bool spawn_tcp_accept_thread(
    const std::shared_ptr<PortTunnelService>& service,
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    SOCKET socket
);
bool spawn_tcp_read_thread(
    const std::shared_ptr<PortTunnelService>& service,
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream
);
bool spawn_udp_read_thread(
    const std::shared_ptr<PortTunnelService>& service,
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
);

class PortTunnelService : public std::enable_shared_from_this<PortTunnelService> {
public:
    explicit PortTunnelService(unsigned long max_workers);

    std::shared_ptr<PortTunnelSession> create_session();
    std::shared_ptr<PortTunnelSession> find_session(const std::string& session_id);
    void attach_session(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<PortTunnelConnection>& connection
    );
    void detach_session(const std::shared_ptr<PortTunnelSession>& session);
    void close_session(const std::shared_ptr<PortTunnelSession>& session);
    bool spawn_tcp_listener_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<RetainedTcpListener>& listener
    );
    bool spawn_udp_bind_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::shared_ptr<TunnelUdpSocket>& socket_value
    );
    bool try_acquire_worker();
    void release_worker();
    unsigned long max_workers() const;

private:
    PortTunnelService(const PortTunnelService&);
    PortTunnelService& operator=(const PortTunnelService&);

    bool schedule_session_expiry(const std::shared_ptr<PortTunnelSession>& session);
    void expire_session_if_needed(const std::shared_ptr<PortTunnelSession>& session);
    std::shared_ptr<PortTunnelConnection> wait_for_attachment(
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
    std::atomic<unsigned long> active_workers_;
    unsigned long max_workers_;
    std::map<std::string, std::shared_ptr<PortTunnelSession> > sessions_;
    std::uint64_t next_session_sequence_;
};

class PortTunnelWorkerLease {
public:
    explicit PortTunnelWorkerLease(const std::shared_ptr<PortTunnelService>& service);
    ~PortTunnelWorkerLease();

private:
    PortTunnelWorkerLease(const PortTunnelWorkerLease&);
    PortTunnelWorkerLease& operator=(const PortTunnelWorkerLease&);

    std::shared_ptr<PortTunnelService> service_;
};

class PortTunnelConnection : public std::enable_shared_from_this<PortTunnelConnection> {
public:
    PortTunnelConnection(SOCKET client, const std::shared_ptr<PortTunnelService>& service)
        : client_(client),
          service_(service),
          closed_(false),
          generation_(0ULL),
          next_daemon_stream_id_(2U) {}

    void run();
    void tcp_accept_loop_transport_owned(uint32_t listener_stream_id, SOCKET listener_socket);
    void tcp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void udp_read_loop_transport_owned(
        uint32_t stream_id,
        std::shared_ptr<TunnelUdpSocket> socket_value
    );
    void send_error(uint32_t stream_id, const std::string& code, const std::string& message);
    void send_terminal_error(
        uint32_t stream_id,
        const std::string& code,
        const std::string& message
    );
    bool owns_session(const std::shared_ptr<PortTunnelSession>& session);
    bool accept_session_tcp_stream(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t listener_stream_id,
        UniqueSocket accepted_socket,
        const std::string& peer
    );
    bool emit_session_udp_datagram(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::string& peer,
        const std::vector<unsigned char>& data
    );

private:
    PortTunnelConnection(const PortTunnelConnection&) = delete;
    PortTunnelConnection& operator=(const PortTunnelConnection&) = delete;

    bool read_exact(unsigned char* data, std::size_t size);
    bool read_preface();
    bool read_frame(PortTunnelFrame* frame);
    void send_frame(const PortTunnelFrame& frame);
    void handle_frame(const PortTunnelFrame& frame);
    void tunnel_open(const PortTunnelFrame& frame);
    void tunnel_close(const PortTunnelFrame& frame);
    void tunnel_heartbeat(const PortTunnelFrame& frame);
    void session_open(const PortTunnelFrame& frame);
    void session_resume(const PortTunnelFrame& frame);
    void tcp_listen(const PortTunnelFrame& frame);
    void tcp_connect(const PortTunnelFrame& frame);
    void tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data);
    void tcp_eof(uint32_t stream_id);
    void udp_bind(const PortTunnelFrame& frame);
    void udp_datagram(const PortTunnelFrame& frame);
    void close_stream(uint32_t stream_id);
    void fail_worker_limit(uint32_t stream_id);
    std::uint64_t current_generation() const;
    void set_generation(std::uint64_t generation);
    void ensure_generation(std::uint64_t frame_generation) const;
    void close_current_session(PortTunnelCloseMode mode);
    void close_transport_owned_state();
    std::shared_ptr<PortTunnelSession> current_session();
    bool session_mode_active();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    BasicMutex writer_mutex_;
    BasicMutex state_mutex_;
    std::atomic<bool> closed_;
    std::atomic<std::uint64_t> generation_;
    std::shared_ptr<PortTunnelSession> session_;
    std::map<uint32_t, UniqueSocket> tcp_listeners_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream> > tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_sockets_;
    std::atomic<uint32_t> next_daemon_stream_id_;
};
