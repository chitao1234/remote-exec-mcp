#include "port_forward/tunnel_service.h"

#include <algorithm>
#include <cerrno>
#include <cstring>
#include <deque>
#include <map>
#include <memory>
#include <string>
#include <vector>

#include "core/blocking_pool.h"
#include "core/byte_buffer.h"
#include "core/event_loop.h"
#include "core/json_value.h"
#include "core/logging.h"
#include "core/text_utils.h"
#include "platform/platform.h"
#include "platform/socket.h"
#include "port_forward/port_forward_endpoint.h"
#include "port_forward/port_forward_socket_ops.h"
#include "port_forward/port_tunnel_frame.h"
#include "rpc/server_contract.h"

namespace {

const std::size_t TUNNEL_INPUT_LIMIT = 512U * 1024U;
const std::size_t TUNNEL_TCP_WRITE_LIMIT = 1024U * 1024U;
const std::size_t TUNNEL_MAX_FRAME_DATA = 256U * 1024U;
const unsigned long TUNNEL_RESUME_TIMEOUT_MS = 10000UL;
const unsigned long TUNNEL_READ_DEADLINE_MS = 30000UL;
const unsigned long TUNNEL_CONNECT_TIMEOUT_MS = 10000UL;
const int TUNNEL_ACCEPTS_PER_EVENT = 32;
const int TUNNEL_DATAGRAMS_PER_EVENT = 32;

enum class TunnelProtocol {
    Tcp,
    Udp,
};

enum class TunnelRole {
    Listen,
    Connect,
};

std::uint64_t now_ms() {
    return platform::monotonic_ms();
}

bool connect_in_progress_error(int error) {
#ifdef _WIN32
    return error == WSAEWOULDBLOCK || error == WSAEINPROGRESS;
#else
    return error == EINPROGRESS || error == EWOULDBLOCK || error == EAGAIN;
#endif
}

std::string meta_string(const PortTunnelFrame& frame, const char* key) {
    try {
        return Json::parse(frame.meta).at(key).get<std::string>();
    } catch (const Json::exception& ex) {
        throw PortTunnelFrameError(std::string("invalid tunnel frame metadata: ") + ex.what());
    }
}

PortTunnelFrame make_frame(
    PortTunnelFrameType type,
    std::uint32_t stream_id,
    const Json& meta,
    const std::string& data
) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta = meta.dump();
    frame.data.assign(data.begin(), data.end());
    return frame;
}

Json limits_json(const PortForwardLimitConfig& limits) {
    return Json{
        {"max_active_tcp_streams", limits.max_active_tcp_streams},
        {"max_udp_peers", limits.max_udp_binds},
        {"max_queued_bytes", limits.max_tunnel_queued_bytes},
    };
}

std::string error_meta(
    const std::string& code,
    const std::string& message,
    bool fatal,
    std::uint64_t generation
) {
    return Json{
        {"code", code},
        {"message", message},
        {"fatal", fatal},
        {"generation", generation},
    }
        .dump();
}

bool contains_upgrade_token(const std::string& value) {
    std::size_t offset = 0U;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset)
        );
        if (lowercase_ascii(token) == "upgrade") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }
    return false;
}

} // namespace

struct TunnelServiceImpl;

struct TunnelSession {
    TunnelSession()
        : protocol(TunnelProtocol::Tcp), attached(nullptr), generation(0U), resume_deadline_ms(0U),
          closed(false) {}

    std::string id;
    TunnelProtocol protocol;
    std::shared_ptr<class TunnelListener> listener;
    std::shared_ptr<class TunnelUdpBind> udp;
    class TunnelConn* attached;
    std::uint64_t generation;
    std::uint64_t resume_deadline_ms;
    bool closed;
};

struct TunnelListener {
    TunnelListener() : listener_stream_id(0U) {}
    UniqueSocket socket;
    std::uint32_t listener_stream_id;
};

struct TunnelUdpBind {
    TunnelUdpBind() : stream_id(0U) {}
    UniqueSocket socket;
    std::uint32_t stream_id;
};

struct TunnelTcpStreamState {
    TunnelTcpStreamState()
        : id(0U), write_offset(0U), read_eof(false), write_eof(false), connecting(false),
          connect_deadline_ms(0U), closed(false) {}

    std::uint32_t id;
    std::shared_ptr<Stream> stream;
    std::string write_buffer;
    std::size_t write_offset;
    bool read_eof;
    bool write_eof;
    bool connecting;
    std::uint64_t connect_deadline_ms;
    bool closed;
};

struct TunnelConn : public std::enable_shared_from_this<TunnelConn> {
    TunnelConn()
        : service(nullptr), id(0U), input(TUNNEL_INPUT_LIMIT), output_offset(0U),
          protocol(TunnelProtocol::Tcp), role(TunnelRole::Connect), generation(0U),
          preface_read(false), mode_set(false), closed(false), read_timer(0U), timer_armed(false),
          read_waiting(false), write_waiting(false), session(), dropped_datagrams(0U),
          next_stream_id(2U) {}

    ~TunnelConn();

    void start();
    void update_interest();
    void on_events(int bits);
    void on_readable();
    void on_writable();
    void process_frames();
    void handle_frame(const PortTunnelFrame& frame);
    bool send_frame(const PortTunnelFrame& frame, bool droppable);
    bool send_error(const std::string& code, const std::string& message, bool fatal);
    void close();
    void attach_session(const std::shared_ptr<TunnelSession>& session, std::uint64_t generation);
    void detach_session();
    void close_stream(std::uint32_t stream_id);
    void close_all_streams();
    void close_all_udp();
    void flush_dropped_datagrams();
    void arm_read_timer();
    std::uint32_t allocate_stream_id();

    TunnelServiceImpl* service;
    std::uint64_t id;
    std::shared_ptr<Stream> transport;
    ByteBuffer input;
    std::string output;
    std::size_t output_offset;
    TunnelProtocol protocol;
    TunnelRole role;
    std::uint64_t generation;
    bool preface_read;
    bool mode_set;
    bool closed;
    EventLoop::TimerId read_timer;
    bool timer_armed;
    bool read_waiting;
    bool write_waiting;
    std::shared_ptr<TunnelSession> session;
    std::map<std::uint32_t, std::shared_ptr<TunnelTcpStreamState>> streams;
    std::map<std::uint32_t, std::shared_ptr<TunnelUdpBind>> udp_binds;
    std::uint64_t dropped_datagrams;
    std::uint32_t next_stream_id;
};

struct TunnelServiceImpl {
    TunnelServiceImpl(
        const PortForwardLimitConfig& limits_value,
        const std::string& bearer_token_value,
        EventLoop* loop_value,
        BlockingPool* pool_value
    )
        : limits(limits_value), bearer_token(bearer_token_value), loop(loop_value),
          pool(pool_value), next_connection_id(1U), next_session_id(1U), retained_sessions(0U),
          retained_listeners(0U), udp_binds(0U), active_tcp_streams(0U), shutting_down(false) {}

    bool try_acquire(unsigned long* counter, unsigned long limit);
    void release(unsigned long* counter);
    std::string make_session_id();
    void adopt(const std::shared_ptr<Stream>& transport, const std::string& initial_input);
    void start_connection(
        const std::shared_ptr<Stream>& transport,
        const std::string& initial_input
    );
    void tick();
    void shutdown();
    void handle_accept(const std::weak_ptr<TunnelSession>& session);
    void handle_udp_session_read(const std::weak_ptr<TunnelSession>& session);
    void handle_udp_conn_read(const std::weak_ptr<TunnelConn>& conn, std::uint32_t stream_id);
    void handle_tcp_read(
        const std::weak_ptr<TunnelConn>& conn,
        const std::weak_ptr<TunnelTcpStreamState>& stream
    );
    void handle_tcp_write(
        const std::weak_ptr<TunnelConn>& conn,
        const std::weak_ptr<TunnelTcpStreamState>& stream
    );
    void expire_sessions();
    void close_session(const std::shared_ptr<TunnelSession>& session, bool mark_closed);
    void detach_sessions_for(const TunnelConn* conn);
    void install_listener(
        const std::shared_ptr<TunnelConn>& conn,
        const std::shared_ptr<TunnelSession>& session,
        std::uint32_t listener_stream_id,
        UniqueSocket socket
    );
    void install_udp(
        const std::shared_ptr<TunnelConn>& conn,
        const std::shared_ptr<TunnelSession>& session,
        std::uint32_t stream_id,
        UniqueSocket socket
    );
    void finish_connect(
        const std::shared_ptr<TunnelConn>& conn,
        const std::shared_ptr<TunnelTcpStreamState>& stream
    );
    void fail_connect(
        const std::shared_ptr<TunnelConn>& conn,
        const std::shared_ptr<TunnelTcpStreamState>& stream,
        const std::string& message
    );

    PortForwardLimitConfig limits;
    std::string bearer_token;
    EventLoop* loop;
    BlockingPool* pool;
    std::map<std::uint64_t, std::shared_ptr<TunnelConn>> connections;
    std::map<std::string, std::shared_ptr<TunnelSession>> sessions;
    std::uint64_t next_connection_id;
    std::uint64_t next_session_id;
    unsigned long retained_sessions;
    unsigned long retained_listeners;
    unsigned long udp_binds;
    unsigned long active_tcp_streams;
    bool shutting_down;
};

TunnelConn::~TunnelConn() {
}

bool TunnelServiceImpl::try_acquire(unsigned long* counter, unsigned long limit) {
    if (*counter >= limit) {
        return false;
    }
    ++(*counter);
    return true;
}

void TunnelServiceImpl::release(unsigned long* counter) {
    if (*counter > 0U) {
        --(*counter);
    }
}

std::string TunnelServiceImpl::make_session_id() {
    return "pts_" + std::to_string(now_ms()) + "_" + std::to_string(next_session_id++);
}

void TunnelServiceImpl::adopt(
    const std::shared_ptr<Stream>& transport,
    const std::string& initial_input
) {
    std::shared_ptr<Stream> captured = transport;
    const std::string initial = initial_input;
    loop->post([this, captured, initial]() { start_connection(captured, initial); });
}

void TunnelServiceImpl::start_connection(
    const std::shared_ptr<Stream>& transport,
    const std::string& initial_input
) {
    if (shutting_down || !transport) {
        if (transport) {
            transport->close();
        }
        return;
    }
    std::shared_ptr<TunnelConn> conn(new TunnelConn());
    conn->service = this;
    conn->id = next_connection_id++;
    conn->transport = transport;
    if (!initial_input.empty() && !conn->input.append(initial_input.data(), initial_input.size())) {
        conn->transport->close();
        return;
    }
    connections[conn->id] = conn;
    conn->start();
}

void TunnelServiceImpl::tick() {
    expire_sessions();

    std::vector<std::shared_ptr<TunnelConn>> conns;
    conns.reserve(connections.size());
    for (std::map<std::uint64_t, std::shared_ptr<TunnelConn>>::const_iterator it =
             connections.begin();
         it != connections.end();
         ++it) {
        conns.push_back(it->second);
    }
    const std::uint64_t now = now_ms();
    for (std::size_t i = 0; i < conns.size(); ++i) {
        const std::shared_ptr<TunnelConn>& conn = conns[i];
        if (conn->closed) {
            continue;
        }
        for (std::map<std::uint32_t, std::shared_ptr<TunnelTcpStreamState>>::iterator it =
                 conn->streams.begin();
             it != conn->streams.end();
             ++it) {
            const std::shared_ptr<TunnelTcpStreamState>& stream = it->second;
            if (stream->connecting && now >= stream->connect_deadline_ms) {
                fail_connect(conn, stream, "port connect timed out");
            }
        }
        conn->flush_dropped_datagrams();
    }
}

void TunnelServiceImpl::shutdown() {
    if (shutting_down) {
        return;
    }
    shutting_down = true;

    std::vector<std::shared_ptr<TunnelConn>> conns;
    conns.reserve(connections.size());
    for (std::map<std::uint64_t, std::shared_ptr<TunnelConn>>::const_iterator it =
             connections.begin();
         it != connections.end();
         ++it) {
        conns.push_back(it->second);
    }
    connections.clear();
    for (std::size_t i = 0; i < conns.size(); ++i) {
        conns[i]->close();
    }

    std::vector<std::shared_ptr<TunnelSession>> sessions_copy;
    sessions_copy.reserve(sessions.size());
    for (std::map<std::string, std::shared_ptr<TunnelSession>>::const_iterator it =
             sessions.begin();
         it != sessions.end();
         ++it) {
        sessions_copy.push_back(it->second);
    }
    sessions.clear();
    for (std::size_t i = 0; i < sessions_copy.size(); ++i) {
        close_session(sessions_copy[i], true);
    }
}

void TunnelServiceImpl::expire_sessions() {
    if (sessions.empty()) {
        return;
    }
    const std::uint64_t now = now_ms();
    std::vector<std::shared_ptr<TunnelSession>> expired;
    for (std::map<std::string, std::shared_ptr<TunnelSession>>::iterator it = sessions.begin();
         it != sessions.end();
         ++it) {
        const std::shared_ptr<TunnelSession>& session = it->second;
        if (session->attached == nullptr && !session->closed && session->resume_deadline_ms != 0U
            && now >= session->resume_deadline_ms) {
            expired.push_back(session);
        }
    }
    for (std::size_t i = 0; i < expired.size(); ++i) {
        log_message(
            LOG_DEBUG,
            "port_tunnel",
            LogMessageBuilder("session expired").quoted_field("session_id", expired[i]->id).str()
        );
        close_session(expired[i], true);
    }
}

void TunnelServiceImpl::close_session(
    const std::shared_ptr<TunnelSession>& session,
    bool mark_closed
) {
    if (!session) {
        return;
    }
    if (session->listener) {
        if (session->listener->socket.valid()) {
            loop->remove(session->listener->socket.get());
            session->listener->socket.reset();
        }
        session->listener.reset();
        release(&retained_listeners);
    }
    if (session->udp) {
        if (session->udp->socket.valid()) {
            loop->remove(session->udp->socket.get());
            session->udp->socket.reset();
        }
        session->udp.reset();
        release(&udp_binds);
    }
    if (mark_closed) {
        session->closed = true;
    }
    session->attached = nullptr;
    std::map<std::string, std::shared_ptr<TunnelSession>>::iterator it = sessions.find(session->id);
    if (it != sessions.end() && it->second == session) {
        sessions.erase(it);
        release(&retained_sessions);
    }
}

void TunnelServiceImpl::detach_sessions_for(const TunnelConn* conn) {
    if (conn == nullptr || !conn->session) {
        return;
    }
    const std::shared_ptr<TunnelSession>& session = conn->session;
    if (session->attached != conn) {
        return;
    }
    session->attached = nullptr;
    session->resume_deadline_ms = now_ms() + TUNNEL_RESUME_TIMEOUT_MS;
    if (session->listener && session->listener->socket.valid()) {
        loop->set_interest(session->listener->socket.get(), 0);
    }
    if (session->udp && session->udp->socket.valid()) {
        loop->set_interest(session->udp->socket.get(), 0);
    }
}

void TunnelServiceImpl::handle_accept(const std::weak_ptr<TunnelSession>& weak_session) {
    std::shared_ptr<TunnelSession> session = weak_session.lock();
    if (!session || session->closed || session->attached == nullptr) {
        return;
    }
    TunnelConn* conn = session->attached;
    std::shared_ptr<TunnelConn> conn_owner;
    for (std::map<std::uint64_t, std::shared_ptr<TunnelConn>>::iterator it = connections.begin();
         it != connections.end();
         ++it) {
        if (it->second.get() == conn) {
            conn_owner = it->second;
            break;
        }
    }
    if (!conn_owner || conn_owner->closed || !session->listener) {
        return;
    }

    for (int i = 0; i < TUNNEL_ACCEPTS_PER_EVENT; ++i) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        SOCKET accepted = accept_port_forward_peer(
            session->listener->socket.get(),
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (accepted == INVALID_SOCKET) {
            const int error = last_socket_error();
            if (would_block_error(error) || error == ECONNABORTED) {
                return;
            }
            return;
        }
        if (!socket_set_nonblocking(accepted, true)) {
            close_socket(accepted);
            continue;
        }
        if (!try_acquire(&active_tcp_streams, limits.max_active_tcp_streams)) {
            close_socket(accepted);
            conn_owner->send_error(
                "port_tunnel_limit_exceeded",
                "port tunnel active tcp stream limit reached",
                false
            );
            continue;
        }

        std::shared_ptr<TunnelTcpStreamState> stream(new TunnelTcpStreamState());
        stream->id = conn_owner->allocate_stream_id();
        stream->stream = make_socket_stream(UniqueSocket(accepted));
        conn_owner->streams[stream->id] = stream;

        const std::weak_ptr<TunnelConn> weak_conn = conn_owner;
        const std::weak_ptr<TunnelTcpStreamState> weak_stream = stream;
        const SOCKET fd = stream->stream->native_fd();
        if (!loop->add(fd, IO_READ, [this, weak_conn, weak_stream](int bits) {
                if ((bits & IO_READ) != 0) {
                    handle_tcp_read(weak_conn, weak_stream);
                }
                if ((bits & IO_WRITE) != 0) {
                    handle_tcp_write(weak_conn, weak_stream);
                }
            })) {
            conn_owner->close_stream(stream->id);
            conn_owner->send_error("port_accept_failed", "unable to watch accepted stream", false);
            continue;
        }

        const std::string peer =
            printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len);
        conn_owner->send_frame(
            make_frame(
                PortTunnelFrameType::TcpAccept,
                stream->id,
                Json{
                    {"listener_stream_id", session->listener->listener_stream_id},
                    {"peer", peer},
                },
                std::string()
            ),
            false
        );
    }
}

void TunnelServiceImpl::handle_tcp_read(
    const std::weak_ptr<TunnelConn>& weak_conn,
    const std::weak_ptr<TunnelTcpStreamState>& weak_stream
) {
    std::shared_ptr<TunnelConn> conn = weak_conn.lock();
    std::shared_ptr<TunnelTcpStreamState> stream = weak_stream.lock();
    if (!conn || !stream || conn->closed || stream->closed || stream->read_eof) {
        return;
    }

    char buffer[64U * 1024U];
    for (int i = 0; i < 4; ++i) {
        const ReadResult result = stream->stream->read(buffer, sizeof(buffer));
        if (result.status == IoStatus::Ok) {
            PortTunnelFrame frame;
            frame.type = PortTunnelFrameType::TcpData;
            frame.flags = 0U;
            frame.stream_id = stream->id;
            frame.data.assign(
                reinterpret_cast<unsigned char*>(buffer),
                reinterpret_cast<unsigned char*>(buffer) + result.bytes
            );
            if (!conn->send_frame(frame, false)) {
                return;
            }
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            return;
        }
        if (result.status == IoStatus::Eof) {
            stream->read_eof = true;
            conn->send_frame(
                make_frame(PortTunnelFrameType::TcpEof, stream->id, Json::object(), std::string()),
                false
            );
            if (stream->write_eof && stream->write_offset >= stream->write_buffer.size()) {
                conn->close_stream(stream->id);
            } else {
                loop->set_interest(
                    stream->stream->native_fd(),
                    stream->write_offset < stream->write_buffer.size() ? IO_WRITE : 0
                );
            }
            return;
        }
        conn->send_error("port_read_failed", "port tunnel tcp read failed", false);
        conn->close_stream(stream->id);
        return;
    }
}

void TunnelServiceImpl::handle_tcp_write(
    const std::weak_ptr<TunnelConn>& weak_conn,
    const std::weak_ptr<TunnelTcpStreamState>& weak_stream
) {
    std::shared_ptr<TunnelConn> conn = weak_conn.lock();
    std::shared_ptr<TunnelTcpStreamState> stream = weak_stream.lock();
    if (!conn || !stream || conn->closed || stream->closed) {
        return;
    }
    if (stream->connecting) {
        int socket_error = 0;
        if (socket_error_option(stream->stream->native_fd(), &socket_error) != 0
            || socket_error != 0) {
            fail_connect(conn, stream, "port connect failed");
            return;
        }
        finish_connect(conn, stream);
        return;
    }

    while (stream->write_offset < stream->write_buffer.size()) {
        const WriteResult result = stream->stream->write(
            stream->write_buffer.data() + stream->write_offset,
            stream->write_buffer.size() - stream->write_offset
        );
        if (result.status == IoStatus::Ok) {
            stream->write_offset += result.bytes;
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            return;
        }
        conn->send_error("port_write_failed", "port tunnel tcp write failed", false);
        conn->close_stream(stream->id);
        return;
    }

    stream->write_buffer.clear();
    stream->write_offset = 0U;
    if (stream->write_eof) {
        shutdown_port_forward_send(stream->stream->native_fd());
    }
    if (stream->read_eof && stream->write_eof) {
        conn->close_stream(stream->id);
        return;
    }
    loop->set_interest(stream->stream->native_fd(), IO_READ);
}

void TunnelServiceImpl::handle_udp_session_read(const std::weak_ptr<TunnelSession>& weak_session) {
    std::shared_ptr<TunnelSession> session = weak_session.lock();
    if (!session || session->closed || session->attached == nullptr || !session->udp) {
        return;
    }
    TunnelConn* conn = session->attached;
    std::shared_ptr<TunnelConn> conn_owner;
    for (std::map<std::uint64_t, std::shared_ptr<TunnelConn>>::iterator it = connections.begin();
         it != connections.end();
         ++it) {
        if (it->second.get() == conn) {
            conn_owner = it->second;
            break;
        }
    }
    if (!conn_owner || conn_owner->closed) {
        return;
    }

    char buffer[64U * 1024U];
    for (int i = 0; i < TUNNEL_DATAGRAMS_PER_EVENT; ++i) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const int received = recv_port_forward_datagram(
            session->udp->socket.get(),
            buffer,
            sizeof(buffer),
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (received <= 0) {
            if (received < 0 && !would_block_error(last_socket_error())) {
                return;
            }
            if (received < 0) {
                return;
            }
            continue;
        }
        const std::string peer =
            printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len);
        PortTunnelFrame frame;
        frame.type = PortTunnelFrameType::UdpDatagram;
        frame.flags = 0U;
        frame.stream_id = session->udp->stream_id;
        frame.meta = Json{{"peer", peer}}.dump();
        frame.data.assign(buffer, buffer + received);
        if (!conn_owner->send_frame(frame, true)) {
            ++conn_owner->dropped_datagrams;
        }
    }
}

void TunnelServiceImpl::handle_udp_conn_read(
    const std::weak_ptr<TunnelConn>& weak_conn,
    std::uint32_t stream_id
) {
    std::shared_ptr<TunnelConn> conn = weak_conn.lock();
    if (!conn || conn->closed) {
        return;
    }
    std::map<std::uint32_t, std::shared_ptr<TunnelUdpBind>>::iterator it =
        conn->udp_binds.find(stream_id);
    if (it == conn->udp_binds.end()) {
        return;
    }
    std::shared_ptr<TunnelUdpBind> bind = it->second;

    char buffer[64U * 1024U];
    for (int i = 0; i < TUNNEL_DATAGRAMS_PER_EVENT; ++i) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const int received = recv_port_forward_datagram(
            bind->socket.get(),
            buffer,
            sizeof(buffer),
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (received < 0) {
            if (would_block_error(last_socket_error())) {
                return;
            }
            return;
        }
        const std::string peer =
            printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len);
        PortTunnelFrame frame;
        frame.type = PortTunnelFrameType::UdpDatagram;
        frame.flags = 0U;
        frame.stream_id = stream_id;
        frame.meta = Json{{"peer", peer}}.dump();
        frame.data.assign(buffer, buffer + received);
        if (!conn->send_frame(frame, true)) {
            ++conn->dropped_datagrams;
        }
    }
}

void TunnelServiceImpl::install_listener(
    const std::shared_ptr<TunnelConn>& conn,
    const std::shared_ptr<TunnelSession>& session,
    std::uint32_t listener_stream_id,
    UniqueSocket socket
) {
    if (conn->closed || session->closed || session->attached != conn.get()) {
        socket.reset();
        release(&retained_listeners);
        return;
    }
    std::shared_ptr<TunnelListener> listener(new TunnelListener());
    listener->socket = std::move(socket);
    listener->listener_stream_id = listener_stream_id;
    session->listener = listener;

    const std::weak_ptr<TunnelSession> weak_session = session;
    if (!loop->add(listener->socket.get(), IO_READ, [this, weak_session](int) {
            handle_accept(weak_session);
        })) {
        listener->socket.reset();
        session->listener.reset();
        release(&retained_listeners);
        conn->send_error("listener_open_failed", "unable to watch tcp listener", false);
        return;
    }
    conn->send_frame(
        make_frame(
            PortTunnelFrameType::TcpListenOk,
            listener_stream_id,
            Json{{"endpoint", socket_local_endpoint(listener->socket.get())}},
            std::string()
        ),
        false
    );
}

void TunnelServiceImpl::install_udp(
    const std::shared_ptr<TunnelConn>& conn,
    const std::shared_ptr<TunnelSession>& session,
    std::uint32_t stream_id,
    UniqueSocket socket
) {
    if (conn->closed) {
        socket.reset();
        release(&udp_binds);
        return;
    }
    std::shared_ptr<TunnelUdpBind> bind(new TunnelUdpBind());
    bind->socket = std::move(socket);
    bind->stream_id = stream_id;

    if (session) {
        if (session->closed || session->attached != conn.get()) {
            bind->socket.reset();
            release(&udp_binds);
            return;
        }
        session->udp = bind;
        const std::weak_ptr<TunnelSession> weak_session = session;
        if (!loop->add(bind->socket.get(), IO_READ, [this, weak_session](int) {
                handle_udp_session_read(weak_session);
            })) {
            bind->socket.reset();
            session->udp.reset();
            release(&udp_binds);
            conn->send_error("listener_open_failed", "unable to watch udp bind", false);
            return;
        }
    } else {
        conn->udp_binds[stream_id] = bind;
        const std::weak_ptr<TunnelConn> weak_conn = conn;
        if (!loop->add(bind->socket.get(), IO_READ, [this, weak_conn, stream_id](int) {
                handle_udp_conn_read(weak_conn, stream_id);
            })) {
            bind->socket.reset();
            conn->udp_binds.erase(stream_id);
            release(&udp_binds);
            conn->send_error("listener_open_failed", "unable to watch udp bind", false);
            return;
        }
    }
    conn->send_frame(
        make_frame(
            PortTunnelFrameType::UdpBindOk,
            stream_id,
            Json{{"endpoint", socket_local_endpoint(bind->socket.get())}},
            std::string()
        ),
        false
    );
}

void TunnelServiceImpl::finish_connect(
    const std::shared_ptr<TunnelConn>& conn,
    const std::shared_ptr<TunnelTcpStreamState>& stream
) {
    if (conn->closed || stream->closed) {
        return;
    }
    stream->connecting = false;
    loop->set_interest(stream->stream->native_fd(), IO_READ);
    conn->send_frame(
        make_frame(PortTunnelFrameType::TcpConnectOk, stream->id, Json::object(), std::string()),
        false
    );

    const std::weak_ptr<TunnelConn> weak_conn = conn;
    const std::weak_ptr<TunnelTcpStreamState> weak_stream = stream;
    const SOCKET fd = stream->stream->native_fd();
    if (!loop->add(fd, IO_READ, [this, weak_conn, weak_stream](int bits) {
            if ((bits & IO_READ) != 0) {
                handle_tcp_read(weak_conn, weak_stream);
            }
            if ((bits & IO_WRITE) != 0) {
                handle_tcp_write(weak_conn, weak_stream);
            }
        })) {
        conn->send_error("port_connect_failed", "unable to watch connected tcp stream", false);
        conn->close_stream(stream->id);
    }
}

void TunnelServiceImpl::fail_connect(
    const std::shared_ptr<TunnelConn>& conn,
    const std::shared_ptr<TunnelTcpStreamState>& stream,
    const std::string& message
) {
    if (conn->closed || stream->closed) {
        return;
    }
    conn->send_error("port_connect_failed", message, false);
    conn->close_stream(stream->id);
}

void TunnelConn::arm_read_timer() {
    if (closed) {
        return;
    }
    if (timer_armed) {
        service->loop->cancel_timer(read_timer);
    }
    std::weak_ptr<TunnelConn> weak = shared_from_this();
    read_timer = service->loop->add_timer(now_ms() + TUNNEL_READ_DEADLINE_MS, [weak]() {
        std::shared_ptr<TunnelConn> conn = weak.lock();
        if (conn) {
            conn->timer_armed = false;
            conn->close();
        }
    });
    timer_armed = true;
}

void TunnelConn::start() {
    std::shared_ptr<TunnelConn> self = shared_from_this();
    if (!service->loop->add(transport->native_fd(), IO_READ, [self](int bits) {
            self->on_events(bits);
        })) {
        close();
        return;
    }
    arm_read_timer();
    if (!input.empty()) {
        process_frames();
    }
}

void TunnelConn::update_interest() {
    if (closed) {
        return;
    }
    int want = transport ? transport->wanted_events() : 0;
    if (want == 0) {
        want = output.empty() ? IO_READ : (IO_READ | IO_WRITE);
    }
    service->loop->set_interest(transport->native_fd(), want);
}

void TunnelConn::on_events(int bits) {
    if (closed) {
        return;
    }
    if (read_waiting || (bits & IO_READ) != 0) {
        on_readable();
    }
    if (closed) {
        return;
    }
    if (write_waiting || (bits & IO_WRITE) != 0 || !output.empty()) {
        on_writable();
    }
}

void TunnelConn::on_readable() {
    char buffer[64U * 1024U];
    for (int i = 0; i < 4; ++i) {
        const ReadResult result = transport->read(buffer, sizeof(buffer));
        if (result.status == IoStatus::Ok) {
            if (!input.append(buffer, result.bytes)) {
                send_error("invalid_port_tunnel", "port tunnel input exceeds limit", true);
                close();
                return;
            }
            arm_read_timer();
            read_waiting = false;
            process_frames();
            if (closed) {
                return;
            }
            update_interest();
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            read_waiting = true;
            update_interest();
            return;
        }
        close();
        return;
    }
}

void TunnelConn::on_writable() {
    while (output_offset < output.size()) {
        const WriteResult result =
            transport->write(output.data() + output_offset, output.size() - output_offset);
        if (result.status == IoStatus::Ok) {
            output_offset += result.bytes;
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            write_waiting = true;
            update_interest();
            return;
        }
        close();
        return;
    }
    write_waiting = false;
    output.clear();
    output_offset = 0U;
    flush_dropped_datagrams();
    if (!closed) {
        update_interest();
    }
}

bool TunnelConn::send_frame(const PortTunnelFrame& frame, bool droppable) {
    if (closed) {
        return false;
    }
    std::vector<unsigned char> bytes;
    try {
        bytes = encode_port_tunnel_frame(frame);
    } catch (const std::exception&) {
        if (!droppable) {
            close();
        }
        return false;
    }
    const std::size_t queued = output.size() - output_offset;
    if (bytes.size() > service->limits.max_tunnel_queued_bytes
        || queued > service->limits.max_tunnel_queued_bytes - bytes.size()) {
        if (droppable) {
            return false;
        }
        close();
        return false;
    }
    output.append(reinterpret_cast<const char*>(bytes.data()), bytes.size());
    if (output_offset == 0U) {
        update_interest();
        on_writable();
    }
    return true;
}

bool TunnelConn::send_error(const std::string& code, const std::string& message, bool fatal) {
    try {
        return send_frame(
            make_frame(
                PortTunnelFrameType::Error,
                0U,
                Json::parse(error_meta(code, message, fatal, generation)),
                std::string()
            ),
            false
        );
    } catch (const std::exception&) {
        return false;
    }
}

void TunnelConn::flush_dropped_datagrams() {
    if (dropped_datagrams == 0U || closed) {
        return;
    }
    const std::uint64_t count = dropped_datagrams;
    dropped_datagrams = 0U;
    send_frame(
        make_frame(
            PortTunnelFrameType::ForwardDrop,
            0U,
            Json{
                {"kind", "udp_datagram"},
                {"count", count},
                {"reason", "port_tunnel_limit_exceeded"},
            },
            std::string()
        ),
        true
    );
}

void TunnelConn::attach_session(
    const std::shared_ptr<TunnelSession>& session_value,
    std::uint64_t generation_value
) {
    session = session_value;
    session->attached = this;
    session->generation = generation_value;
    session->resume_deadline_ms = 0U;
    generation = generation_value;
    if (session->listener && session->listener->socket.valid()) {
        service->loop->set_interest(session->listener->socket.get(), IO_READ);
    }
    if (session->udp && session->udp->socket.valid()) {
        service->loop->set_interest(session->udp->socket.get(), IO_READ);
    }
}

void TunnelConn::detach_session() {
    if (!session || session->attached != this) {
        return;
    }
    service->detach_sessions_for(this);
}

std::uint32_t TunnelConn::allocate_stream_id() {
    const std::uint32_t id = next_stream_id;
    next_stream_id += 2U;
    if (next_stream_id == 0U) {
        close();
    }
    return id;
}

void TunnelConn::close_stream(std::uint32_t stream_id) {
    std::map<std::uint32_t, std::shared_ptr<TunnelTcpStreamState>>::iterator it =
        streams.find(stream_id);
    if (it == streams.end()) {
        return;
    }
    std::shared_ptr<TunnelTcpStreamState> stream = it->second;
    streams.erase(it);
    if (stream && !stream->closed) {
        stream->closed = true;
        if (stream->stream) {
            service->loop->remove(stream->stream->native_fd());
            stream->stream->close();
        }
        service->release(&service->active_tcp_streams);
    }
}

void TunnelConn::close_all_streams() {
    std::vector<std::uint32_t> ids;
    ids.reserve(streams.size());
    for (std::map<std::uint32_t, std::shared_ptr<TunnelTcpStreamState>>::const_iterator it =
             streams.begin();
         it != streams.end();
         ++it) {
        ids.push_back(it->first);
    }
    for (std::size_t i = 0; i < ids.size(); ++i) {
        close_stream(ids[i]);
    }
}

void TunnelConn::close_all_udp() {
    for (std::map<std::uint32_t, std::shared_ptr<TunnelUdpBind>>::iterator it = udp_binds.begin();
         it != udp_binds.end();
         ++it) {
        if (it->second && it->second->socket.valid()) {
            service->loop->remove(it->second->socket.get());
            it->second->socket.reset();
        }
        service->release(&service->udp_binds);
    }
    udp_binds.clear();
}

void TunnelConn::close() {
    if (closed) {
        return;
    }
    closed = true;
    if (timer_armed) {
        service->loop->cancel_timer(read_timer);
        timer_armed = false;
    }
    service->loop->remove(transport->native_fd());
    close_all_streams();
    close_all_udp();
    detach_session();
    if (transport) {
        transport->close();
    }
    std::map<std::uint64_t, std::shared_ptr<TunnelConn>>::iterator it =
        service->connections.find(id);
    if (it != service->connections.end() && it->second.get() == this) {
        service->connections.erase(it);
    }
}

void TunnelConn::process_frames() {
    if (!preface_read) {
        const std::size_t preface_size = port_tunnel_preface_size();
        if (input.size() < preface_size) {
            return;
        }
        if (std::memcmp(input.data(), port_tunnel_preface(), preface_size) != 0) {
            send_error("invalid_port_tunnel", "invalid port tunnel preface", true);
            close();
            return;
        }
        input.consume(preface_size);
        preface_read = true;
    }

    for (;;) {
        if (closed) {
            return;
        }
        if (input.size() < PORT_TUNNEL_HEADER_LEN) {
            return;
        }
        const unsigned char* header = reinterpret_cast<const unsigned char*>(input.data());
        const std::uint32_t meta_len = (static_cast<std::uint32_t>(header[8]) << 24)
                                       | (static_cast<std::uint32_t>(header[9]) << 16)
                                       | (static_cast<std::uint32_t>(header[10]) << 8)
                                       | static_cast<std::uint32_t>(header[11]);
        const std::uint32_t data_len = (static_cast<std::uint32_t>(header[12]) << 24)
                                       | (static_cast<std::uint32_t>(header[13]) << 16)
                                       | (static_cast<std::uint32_t>(header[14]) << 8)
                                       | static_cast<std::uint32_t>(header[15]);
        if (meta_len > PORT_TUNNEL_MAX_META_LEN || data_len > PORT_TUNNEL_MAX_DATA_LEN) {
            send_error("invalid_port_tunnel", "port tunnel frame exceeds maximum length", true);
            close();
            return;
        }
        const std::size_t total =
            PORT_TUNNEL_HEADER_LEN + static_cast<std::size_t>(meta_len) + data_len;
        if (input.size() < total) {
            return;
        }
        std::vector<unsigned char> bytes(
            reinterpret_cast<const unsigned char*>(input.data()),
            reinterpret_cast<const unsigned char*>(input.data()) + total
        );
        input.consume(total);

        PortTunnelFrame frame;
        try {
            frame = decode_port_tunnel_frame(bytes);
        } catch (const std::exception& ex) {
            send_error("invalid_port_tunnel", ex.what(), true);
            close();
            return;
        }
        try {
            handle_frame(frame);
        } catch (const std::exception& ex) {
            send_error("invalid_port_tunnel", ex.what(), false);
        }
    }
}

void TunnelConn::handle_frame(const PortTunnelFrame& frame) {
    switch (frame.type) {
    case PortTunnelFrameType::TunnelOpen: {
        if (mode_set || session) {
            send_error("port_tunnel_already_attached", "port tunnel is already open", true);
            return;
        }
        const Json meta = Json::parse(frame.meta);
        const std::string role_value = meta.at("role").get<std::string>();
        const std::string protocol_value = meta.at("protocol").get<std::string>();
        const std::uint64_t open_generation = meta.at("generation").get<std::uint64_t>();
        if (protocol_value == "tcp") {
            protocol = TunnelProtocol::Tcp;
        } else if (protocol_value == "udp") {
            protocol = TunnelProtocol::Udp;
        } else {
            send_error("invalid_port_tunnel", "unknown tunnel protocol", true);
            return;
        }
        mode_set = true;
        if (role_value == "connect") {
            role = TunnelRole::Connect;
            generation = open_generation;
            send_frame(
                make_frame(
                    PortTunnelFrameType::TunnelReady,
                    0U,
                    Json{
                        {"generation", generation},
                        {"limits", limits_json(service->limits)},
                    },
                    std::string()
                ),
                false
            );
            return;
        }
        if (role_value != "listen") {
            send_error("invalid_port_tunnel", "unknown tunnel role", true);
            return;
        }
        role = TunnelRole::Listen;
        std::shared_ptr<TunnelSession> session_value;
        const bool resume =
            meta.contains("resume_session_id") && !meta.at("resume_session_id").is_null();
        if (resume) {
            const std::string resume_id = meta.at("resume_session_id").get<std::string>();
            std::map<std::string, std::shared_ptr<TunnelSession>>::iterator it =
                service->sessions.find(resume_id);
            if (it == service->sessions.end()) {
                send_error("unknown_port_tunnel_session", "unknown port tunnel session", true);
                return;
            }
            session_value = it->second;
            if (session_value->closed) {
                send_error("port_tunnel_resume_expired", "port tunnel resume expired", true);
                return;
            }
            if (session_value->attached != nullptr) {
                send_error(
                    "port_tunnel_already_attached",
                    "port tunnel session is already attached",
                    true
                );
                return;
            }
            if (session_value->protocol != protocol) {
                send_error("invalid_port_tunnel", "port tunnel protocol mismatch", true);
                return;
            }
        } else {
            if (!service->try_acquire(
                    &service->retained_sessions,
                    service->limits.max_retained_sessions
                )) {
                send_error(
                    "port_tunnel_limit_exceeded",
                    "port tunnel retained session limit reached",
                    true
                );
                return;
            }
            session_value.reset(new TunnelSession());
            session_value->id = service->make_session_id();
            session_value->protocol = protocol;
            service->sessions[session_value->id] = session_value;
        }
        attach_session(session_value, open_generation);
        send_frame(
            make_frame(
                PortTunnelFrameType::TunnelReady,
                0U,
                Json{
                    {"generation", generation},
                    {"session_id", session_value->id},
                    {"resume_timeout_ms", TUNNEL_RESUME_TIMEOUT_MS},
                    {"limits", limits_json(service->limits)},
                },
                std::string()
            ),
            false
        );
        return;
    }
    case PortTunnelFrameType::TunnelClose:
        send_frame(
            make_frame(
                PortTunnelFrameType::TunnelClosed,
                0U,
                Json::parse(frame.meta),
                std::string()
            ),
            false
        );
        if (session) {
            service->close_session(session, true);
            session.reset();
        }
        close();
        return;
    case PortTunnelFrameType::TunnelHeartbeat:
        send_frame(
            make_frame(
                PortTunnelFrameType::TunnelHeartbeatAck,
                0U,
                Json::parse(frame.meta),
                std::string()
            ),
            false
        );
        return;
    case PortTunnelFrameType::TcpListen: {
        if (role != TunnelRole::Listen || protocol != TunnelProtocol::Tcp || !session) {
            send_error("invalid_port_tunnel", "tcp listen requires an open listen tunnel", false);
            return;
        }
        if (session->listener || session->udp) {
            send_error("invalid_port_tunnel", "listen session already has a resource", false);
            return;
        }
        const std::string endpoint =
            normalize_port_forward_endpoint(meta_string(frame, "endpoint"));
        const std::uint32_t listener_stream_id = frame.stream_id;
        if (!service->try_acquire(
                &service->retained_listeners,
                service->limits.max_retained_listeners
            )) {
            send_error(
                "port_tunnel_limit_exceeded",
                "port tunnel retained listener limit reached",
                false
            );
            return;
        }
        std::shared_ptr<TunnelConn> self = shared_from_this();
        std::shared_ptr<TunnelSession> session_value = session;
        const bool posted =
            service->pool->post([this, self, session_value, endpoint, listener_stream_id]() {
                SOCKET listener = INVALID_SOCKET;
                std::string error;
                try {
                    listener = bind_port_forward_socket(endpoint, "tcp");
                    if (listener != INVALID_SOCKET && !socket_set_nonblocking(listener, true)) {
                        close_socket(listener);
                        listener = INVALID_SOCKET;
                        error = "failed to set tcp listener non-blocking";
                    }
                } catch (const std::exception& ex) {
                    error = ex.what();
                }
                service->loop->post([this, self, session_value, listener_stream_id, listener]() {
                    if (listener == INVALID_SOCKET) {
                        self->send_error(
                            "listener_open_failed",
                            "unable to open tcp listener",
                            false
                        );
                        service->release(&service->retained_listeners);
                        return;
                    }
                    if (self->closed || self->session.get() != session_value.get()) {
                        close_socket(listener);
                        service->release(&service->retained_listeners);
                        return;
                    }
                    service->install_listener(
                        self,
                        session_value,
                        listener_stream_id,
                        UniqueSocket(listener)
                    );
                });
            });
        if (!posted) {
            service->release(&service->retained_listeners);
            send_error("port_tunnel_limit_exceeded", "tunnel worker pool is full", false);
        }
        return;
    }
    case PortTunnelFrameType::TcpConnect: {
        if (protocol != TunnelProtocol::Tcp) {
            send_error("invalid_port_tunnel", "tcp connect requires a tcp tunnel", false);
            return;
        }
        if (!service->try_acquire(
                &service->active_tcp_streams,
                service->limits.max_active_tcp_streams
            )) {
            send_error(
                "port_tunnel_limit_exceeded",
                "port tunnel active tcp stream limit reached",
                false
            );
            return;
        }
        const std::string endpoint =
            ensure_nonzero_connect_endpoint(meta_string(frame, "endpoint"));
        const std::uint32_t stream_id = frame.stream_id;
        std::shared_ptr<TunnelConn> self = shared_from_this();
        const bool posted = service->pool->post([this, self, endpoint, stream_id]() {
            ParsedPortForwardEndpoint parsed;
            std::vector<SocketAddress> addresses;
            std::string error;
            try {
                parsed = parse_port_forward_endpoint(endpoint);
                SocketAddressQuery query;
                query.family = AF_UNSPEC;
                query.socktype = SOCK_STREAM;
                query.protocol = IPPROTO_TCP;
                query.passive = false;
                if (!resolve_socket_addresses(
                        parsed.host.c_str(),
                        parsed.port.c_str(),
                        query,
                        &addresses,
                        &error
                    )) {
                    addresses.clear();
                }
            } catch (const std::exception& ex) {
                error = ex.what();
                addresses.clear();
            }
            service->loop->post([this, self, addresses, stream_id, error]() {
                if (self->closed) {
                    service->release(&service->active_tcp_streams);
                    return;
                }
                if (addresses.empty()) {
                    self->send_error(
                        "port_connect_failed",
                        error.empty() ? "unable to resolve endpoint" : error,
                        false
                    );
                    service->release(&service->active_tcp_streams);
                    return;
                }

                SOCKET raw = INVALID_SOCKET;
                int pending_error = 0;
                for (std::size_t i = 0; i < addresses.size(); ++i) {
                    raw = create_socket_cloexec(
                        addresses[i].family,
                        addresses[i].socktype,
                        addresses[i].protocol
                    );
                    if (raw == INVALID_SOCKET) {
                        continue;
                    }
                    if (!socket_set_nonblocking(raw, true)) {
                        close_socket(raw);
                        raw = INVALID_SOCKET;
                        continue;
                    }
                    const int result =
                        connect_socket(raw, addresses[i].sockaddr_ptr(), addresses[i].address_len);
                    if (result == 0) {
                        pending_error = 0;
                        break;
                    }
                    const int error_code = last_socket_error();
                    if (connect_in_progress_error(error_code)) {
                        pending_error = 1;
                        break;
                    }
                    pending_error = error_code;
                    close_socket(raw);
                    raw = INVALID_SOCKET;
                }
                if (raw == INVALID_SOCKET) {
                    self->send_error("port_connect_failed", "unable to connect endpoint", false);
                    service->release(&service->active_tcp_streams);
                    return;
                }

                std::shared_ptr<TunnelTcpStreamState> stream(new TunnelTcpStreamState());
                stream->id = stream_id;
                stream->stream = make_socket_stream(UniqueSocket(raw));
                self->streams[stream_id] = stream;

                const std::weak_ptr<TunnelConn> weak_conn = self;
                const std::weak_ptr<TunnelTcpStreamState> weak_stream = stream;
                const SOCKET fd = stream->stream->native_fd();
                if (pending_error == 0) {
                    service->finish_connect(self, stream);
                } else {
                    stream->connecting = true;
                    stream->connect_deadline_ms = now_ms() + TUNNEL_CONNECT_TIMEOUT_MS;
                    service->loop->add(fd, IO_WRITE, [this, weak_conn, weak_stream](int bits) {
                        if ((bits & IO_WRITE) != 0) {
                            service->handle_tcp_write(weak_conn, weak_stream);
                        }
                    });
                }
            });
        });
        if (!posted) {
            service->release(&service->active_tcp_streams);
            send_error("port_tunnel_limit_exceeded", "tunnel worker pool is full", false);
        }
        return;
    }
    case PortTunnelFrameType::TcpData: {
        if (role != TunnelRole::Listen && role != TunnelRole::Connect) {
            return;
        }
        std::map<std::uint32_t, std::shared_ptr<TunnelTcpStreamState>>::iterator it =
            streams.find(frame.stream_id);
        if (it == streams.end()) {
            send_error("unknown_port_connection", "unknown tunnel tcp stream", false);
            return;
        }
        const std::shared_ptr<TunnelTcpStreamState>& stream = it->second;
        const std::size_t pending = stream->write_buffer.size() - stream->write_offset;
        if (pending + frame.data.size() > TUNNEL_TCP_WRITE_LIMIT) {
            send_error(
                "port_tunnel_limit_exceeded",
                "port tunnel tcp write queue limit reached",
                false
            );
            close_stream(frame.stream_id);
            return;
        }
        stream->write_buffer.append(
            reinterpret_cast<const char*>(frame.data.data()),
            frame.data.size()
        );
        service->loop->set_interest(stream->stream->native_fd(), IO_READ | IO_WRITE);
        return;
    }
    case PortTunnelFrameType::TcpEof: {
        std::map<std::uint32_t, std::shared_ptr<TunnelTcpStreamState>>::iterator it =
            streams.find(frame.stream_id);
        if (it == streams.end()) {
            return;
        }
        const std::shared_ptr<TunnelTcpStreamState>& stream = it->second;
        stream->write_eof = true;
        if (stream->write_offset >= stream->write_buffer.size()) {
            shutdown_port_forward_send(stream->stream->native_fd());
            if (stream->read_eof) {
                close_stream(frame.stream_id);
            }
        }
        return;
    }
    case PortTunnelFrameType::Close:
        close_stream(frame.stream_id);
        {
            std::map<std::uint32_t, std::shared_ptr<TunnelUdpBind>>::iterator it =
                udp_binds.find(frame.stream_id);
            if (it != udp_binds.end()) {
                if (it->second && it->second->socket.valid()) {
                    service->loop->remove(it->second->socket.get());
                    it->second->socket.reset();
                }
                udp_binds.erase(it);
                service->release(&service->udp_binds);
            }
        }
        return;
    case PortTunnelFrameType::UdpBind: {
        if (protocol != TunnelProtocol::Udp) {
            send_error("invalid_port_tunnel", "udp bind requires a udp tunnel", false);
            return;
        }
        if (session && (session->listener || session->udp)) {
            send_error("invalid_port_tunnel", "listen session already has a resource", false);
            return;
        }
        const std::string endpoint =
            normalize_port_forward_endpoint(meta_string(frame, "endpoint"));
        const std::uint32_t stream_id = frame.stream_id;
        if (!service->try_acquire(&service->udp_binds, service->limits.max_udp_binds)) {
            send_error("port_tunnel_limit_exceeded", "port tunnel udp bind limit reached", false);
            return;
        }
        std::shared_ptr<TunnelConn> self = shared_from_this();
        std::shared_ptr<TunnelSession> session_value = session;
        const bool posted = service->pool->post([this, self, session_value, endpoint, stream_id]() {
            SOCKET socket = INVALID_SOCKET;
            std::string error;
            try {
                socket = bind_port_forward_socket(endpoint, "udp");
                if (socket != INVALID_SOCKET && !socket_set_nonblocking(socket, true)) {
                    close_socket(socket);
                    socket = INVALID_SOCKET;
                    error = "failed to set udp bind non-blocking";
                }
            } catch (const std::exception& ex) {
                error = ex.what();
            }
            service->loop->post([this, self, session_value, stream_id, socket]() {
                if (socket == INVALID_SOCKET) {
                    self->send_error("listener_open_failed", "unable to open udp bind", false);
                    service->release(&service->udp_binds);
                    return;
                }
                if (self->closed || (session_value && self->session.get() != session_value.get())) {
                    close_socket(socket);
                    service->release(&service->udp_binds);
                    return;
                }
                service->install_udp(self, session_value, stream_id, UniqueSocket(socket));
            });
        });
        if (!posted) {
            service->release(&service->udp_binds);
            send_error("port_tunnel_limit_exceeded", "tunnel worker pool is full", false);
        }
        return;
    }
    case PortTunnelFrameType::UdpDatagram: {
        std::shared_ptr<TunnelUdpBind> bind;
        if (session && session->udp && session->udp->stream_id == frame.stream_id
            && session->attached == this) {
            bind = session->udp;
            if (session->listener && session->listener->socket.valid()) {
                service->loop->set_interest(session->listener->socket.get(), IO_READ);
            }
            if (session->udp && session->udp->socket.valid()) {
                service->loop->set_interest(session->udp->socket.get(), IO_READ);
            }
        } else {
            std::map<std::uint32_t, std::shared_ptr<TunnelUdpBind>>::iterator it =
                udp_binds.find(frame.stream_id);
            if (it == udp_binds.end()) {
                return;
            }
            bind = it->second;
        }
        if (!bind || !bind->socket.valid()) {
            return;
        }
        const std::string peer = meta_string(frame, "peer");
        const SocketAddress peer_address = parse_port_forward_peer(peer);
        (void)send_port_forward_datagram(
            bind->socket.get(),
            reinterpret_cast<const char*>(frame.data.data()),
            frame.data.size(),
            peer_address.sockaddr_ptr(),
            peer_address.address_len
        );
        return;
    }
    case PortTunnelFrameType::Error:
    case PortTunnelFrameType::ForwardDrop:
        return;
    default:
        send_error("invalid_port_tunnel", "unexpected frame from broker", false);
        return;
    }
}

TunnelService::TunnelService(
    const PortForwardLimitConfig& limits,
    const std::string& bearer_token,
    EventLoop* loop,
    BlockingPool* pool
)
    : impl_(new TunnelServiceImpl(limits, bearer_token, loop, pool)) {
}

TunnelService::~TunnelService() {
    shutdown();
}

bool TunnelService::validate_upgrade(const HttpRequest& request, HttpResponse* error) const {
    if (!impl_->bearer_token.empty() && !request_has_bearer_auth(request, impl_->bearer_token)) {
        write_bearer_auth_challenge(*error);
        return false;
    }
    if (request.method != "POST" || request.path != "/v1/port/tunnel"
        || !contains_upgrade_token(request.header("connection"))
        || lowercase_ascii(request.header("upgrade")) != server_contract::PORT_TUNNEL_UPGRADE_TOKEN
        || request.header(server_contract::PORT_TUNNEL_VERSION_HEADER)
               != server_contract::PORT_TUNNEL_VERSION_VALUE) {
        write_rpc_error(*error, 400, "bad_request", "invalid port tunnel upgrade request");
        return false;
    }
    return true;
}

void TunnelService::adopt(
    const std::shared_ptr<Stream>& transport,
    const std::string& initial_input
) {
    impl_->adopt(transport, initial_input);
}

void TunnelService::tick() {
    impl_->tick();
}

void TunnelService::shutdown() {
    if (impl_) {
        impl_->shutdown();
    }
}
