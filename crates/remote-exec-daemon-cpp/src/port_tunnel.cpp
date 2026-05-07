#include "port_tunnel.h"

#include <atomic>
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
#include "http_helpers.h"
#include "json.hpp"
#include "logging.h"
#include "platform.h"
#include "port_forward_endpoint.h"
#include "port_forward_error.h"
#include "port_forward_socket_ops.h"
#include "port_tunnel_frame.h"
#include "server_transport.h"
#include "text_utils.h"

using Json = nlohmann::json;

namespace {

const std::size_t READ_BUF_SIZE = 64U * 1024U;
const unsigned long RETAINED_SOCKET_POLL_TIMEOUT_MS = 100UL;
#ifdef REMOTE_EXEC_CPP_TESTING
const unsigned long RESUME_TIMEOUT_MS = 100UL;
#else
const unsigned long RESUME_TIMEOUT_MS = 10000UL;
#endif

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

struct PortTunnelSession {
    explicit PortTunnelSession(const std::string& session_id_value)
        : session_id(session_id_value),
          attached(false),
          closed(false),
          expired(false),
          resume_deadline_ms(0ULL),
          next_daemon_stream_id(2U) {}

    std::string session_id;
    BasicMutex mutex;
    BasicCondVar state_changed;
    bool attached;
    bool closed;
    bool expired;
    std::uint64_t resume_deadline_ms;
    std::weak_ptr<PortTunnelConnection> connection;
    std::map<uint32_t, std::shared_ptr<RetainedTcpListener> > tcp_listeners;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_binds;
    std::uint32_t next_daemon_stream_id;
};

std::string header_token_lower(const HttpRequest& request, const std::string& name) {
    return lowercase_ascii(request.header(name));
}

bool connection_header_has_upgrade(const HttpRequest& request) {
    const std::string value = header_token_lower(request, "connection");
    std::size_t offset = 0;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos ? value.substr(offset) : value.substr(offset, comma - offset)
        );
        if (token == "upgrade") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }
    return false;
}

std::string frame_meta_string(const PortTunnelFrame& frame, const std::string& key) {
    return Json::parse(frame.meta).at(key).get<std::string>();
}

PortTunnelFrame make_empty_frame(PortTunnelFrameType type, uint32_t stream_id) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    return frame;
}

void mark_tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream) {
    BasicLockGuard lock(stream->mutex);
    if (!stream->closed) {
        stream->closed = true;
        shutdown_socket(stream->socket.get());
        stream->socket.reset();
    }
}

void mark_udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(socket_value->mutex);
    if (!socket_value->closed) {
        socket_value->closed = true;
        shutdown_socket(socket_value->socket.get());
        socket_value->socket.reset();
    }
}

bool tcp_stream_closed(const std::shared_ptr<TunnelTcpStream>& stream) {
    BasicLockGuard lock(stream->mutex);
    return stream->closed;
}

bool udp_socket_closed(const std::shared_ptr<TunnelUdpSocket>& socket_value) {
    BasicLockGuard lock(socket_value->mutex);
    return socket_value->closed;
}

bool retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(listener->mutex);
    return listener->closed;
}

bool session_is_unavailable(const std::shared_ptr<PortTunnelSession>& session) {
    BasicLockGuard lock(session->mutex);
    return session->closed || session->expired;
}

int wait_socket_readable(SOCKET socket, unsigned long timeout_ms) {
    fd_set readfds;
    FD_ZERO(&readfds);
    FD_SET(socket, &readfds);

    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);

#ifdef _WIN32
    return select(0, &readfds, NULL, NULL, &timeout);
#else
    return select(socket + 1, &readfds, NULL, NULL, &timeout);
#endif
}

void mark_retained_listener_closed(const std::shared_ptr<RetainedTcpListener>& listener) {
    BasicLockGuard lock(listener->mutex);
    if (!listener->closed) {
        listener->closed = true;
        shutdown_socket(listener->listener.get());
        listener->listener.reset();
    }
}

}  // namespace

class PortTunnelService : public std::enable_shared_from_this<PortTunnelService> {
public:
    PortTunnelService() : next_session_sequence_(1ULL) {}

    std::shared_ptr<PortTunnelSession> create_session();
    std::shared_ptr<PortTunnelSession> find_session(const std::string& session_id);
    void attach_session(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<PortTunnelConnection>& connection
    );
    void detach_session(const std::shared_ptr<PortTunnelSession>& session);
    void close_session(const std::shared_ptr<PortTunnelSession>& session);
    void spawn_tcp_listener_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        const std::shared_ptr<RetainedTcpListener>& listener
    );
    void spawn_udp_bind_loop(
        const std::shared_ptr<PortTunnelSession>& session,
        uint32_t stream_id,
        const std::shared_ptr<TunnelUdpSocket>& socket_value
    );

private:
    PortTunnelService(const PortTunnelService&);
    PortTunnelService& operator=(const PortTunnelService&);

    void schedule_session_expiry(const std::shared_ptr<PortTunnelSession>& session);
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
    std::map<std::string, std::shared_ptr<PortTunnelSession> > sessions_;
    std::uint64_t next_session_sequence_;
};

namespace {

class PortTunnelConnection : public std::enable_shared_from_this<PortTunnelConnection> {
public:
    PortTunnelConnection(SOCKET client, const std::shared_ptr<PortTunnelService>& service)
        : client_(client),
          service_(service),
          closed_(false),
          next_daemon_stream_id_(2U) {}

    void run();
    void tcp_accept_loop_transport_owned(uint32_t listener_stream_id, SOCKET listener_socket);
    void tcp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void udp_read_loop_transport_owned(
        uint32_t stream_id,
        std::shared_ptr<TunnelUdpSocket> socket_value
    );
    void send_error(uint32_t stream_id, const std::string& code, const std::string& message);
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
    void session_open(const PortTunnelFrame& frame);
    void session_resume(const PortTunnelFrame& frame);
    void tcp_listen(const PortTunnelFrame& frame);
    void tcp_connect(const PortTunnelFrame& frame);
    void tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data);
    void tcp_eof(uint32_t stream_id);
    void udp_bind(const PortTunnelFrame& frame);
    void udp_datagram(const PortTunnelFrame& frame);
    void close_stream(uint32_t stream_id);
    void detach_session();
    void close_transport_owned_state();
    std::shared_ptr<PortTunnelSession> current_session();
    bool session_mode_active();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    BasicMutex writer_mutex_;
    BasicMutex state_mutex_;
    std::atomic<bool> closed_;
    std::shared_ptr<PortTunnelSession> session_;
    std::map<uint32_t, UniqueSocket> tcp_listeners_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream> > tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_sockets_;
    std::atomic<uint32_t> next_daemon_stream_id_;
};

#ifdef _WIN32
struct TcpAcceptContext {
    std::shared_ptr<PortTunnelConnection>* tunnel;
    uint32_t stream_id;
    SOCKET socket;
};

DWORD WINAPI tcp_accept_thread_entry(LPVOID raw_context) {
    std::unique_ptr<TcpAcceptContext> context(static_cast<TcpAcceptContext*>(raw_context));
    std::shared_ptr<PortTunnelConnection> tunnel(*context->tunnel);
    delete context->tunnel;
    tunnel->tcp_accept_loop_transport_owned(context->stream_id, context->socket);
    return 0;
}
#endif

void spawn_tcp_accept_thread(
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    SOCKET socket
) {
#ifdef _WIN32
    std::unique_ptr<TcpAcceptContext> context(new TcpAcceptContext());
    context->tunnel = new std::shared_ptr<PortTunnelConnection>(tunnel);
    context->stream_id = stream_id;
    context->socket = socket;
    HANDLE handle = CreateThread(NULL, 0, tcp_accept_thread_entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::thread([tunnel, stream_id, socket]() {
        tunnel->tcp_accept_loop_transport_owned(stream_id, socket);
    }).detach();
#endif
}

#ifdef _WIN32
struct TcpReadContext {
    std::shared_ptr<PortTunnelConnection>* tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelTcpStream>* stream;
};

DWORD WINAPI tcp_read_thread_entry(LPVOID raw_context) {
    std::unique_ptr<TcpReadContext> context(static_cast<TcpReadContext*>(raw_context));
    std::shared_ptr<PortTunnelConnection> tunnel(*context->tunnel);
    std::shared_ptr<TunnelTcpStream> stream(*context->stream);
    delete context->tunnel;
    delete context->stream;
    tunnel->tcp_read_loop(context->stream_id, stream);
    return 0;
}
#endif

void spawn_tcp_read_thread(
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelTcpStream>& stream
) {
#ifdef _WIN32
    std::unique_ptr<TcpReadContext> context(new TcpReadContext());
    context->tunnel = new std::shared_ptr<PortTunnelConnection>(tunnel);
    context->stream_id = stream_id;
    context->stream = new std::shared_ptr<TunnelTcpStream>(stream);
    HANDLE handle = CreateThread(NULL, 0, tcp_read_thread_entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::thread([tunnel, stream_id, stream]() { tunnel->tcp_read_loop(stream_id, stream); }).detach();
#endif
}

#ifdef _WIN32
struct UdpReadContext {
    std::shared_ptr<PortTunnelConnection>* tunnel;
    uint32_t stream_id;
    std::shared_ptr<TunnelUdpSocket>* socket_value;
};

DWORD WINAPI udp_read_thread_entry(LPVOID raw_context) {
    std::unique_ptr<UdpReadContext> context(static_cast<UdpReadContext*>(raw_context));
    std::shared_ptr<PortTunnelConnection> tunnel(*context->tunnel);
    std::shared_ptr<TunnelUdpSocket> socket_value(*context->socket_value);
    delete context->tunnel;
    delete context->socket_value;
    tunnel->udp_read_loop_transport_owned(context->stream_id, socket_value);
    return 0;
}
#endif

void spawn_udp_read_thread(
    const std::shared_ptr<PortTunnelConnection>& tunnel,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
#ifdef _WIN32
    std::unique_ptr<UdpReadContext> context(new UdpReadContext());
    context->tunnel = new std::shared_ptr<PortTunnelConnection>(tunnel);
    context->stream_id = stream_id;
    context->socket_value = new std::shared_ptr<TunnelUdpSocket>(socket_value);
    HANDLE handle = CreateThread(NULL, 0, udp_read_thread_entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::thread([tunnel, stream_id, socket_value]() {
        tunnel->udp_read_loop_transport_owned(stream_id, socket_value);
    }).detach();
#endif
}

}  // namespace

bool is_port_tunnel_upgrade_request(const HttpRequest& request) {
    return request.method == "POST" && request.path == "/v1/port/tunnel";
}

std::shared_ptr<PortTunnelService> create_port_tunnel_service() {
    return std::shared_ptr<PortTunnelService>(new PortTunnelService());
}

std::shared_ptr<PortTunnelSession> PortTunnelService::create_session() {
    std::shared_ptr<PortTunnelSession> session;
    {
        BasicLockGuard lock(mutex_);
        std::ostringstream out;
        out << "sess_cpp_" << platform::monotonic_ms() << "_" << next_session_sequence_++;
        session.reset(new PortTunnelSession(out.str()));
        sessions_[session->session_id] = session;
    }
    return session;
}

std::shared_ptr<PortTunnelSession> PortTunnelService::find_session(
    const std::string& session_id
) {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<PortTunnelSession> >::iterator it =
        sessions_.find(session_id);
    if (it == sessions_.end()) {
        return std::shared_ptr<PortTunnelSession>();
    }
    return it->second;
}

void PortTunnelService::attach_session(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<PortTunnelConnection>& connection
) {
    BasicLockGuard lock(session->mutex);
    session->attached = true;
    session->closed = false;
    session->expired = false;
    session->resume_deadline_ms = 0ULL;
    session->connection = connection;
    session->state_changed.broadcast();
}

void PortTunnelService::detach_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired) {
            return;
        }
        session->attached = false;
        session->resume_deadline_ms = platform::monotonic_ms() + RESUME_TIMEOUT_MS;
        session->connection.reset();
        session->state_changed.broadcast();
    }
    schedule_session_expiry(session);
}

void PortTunnelService::close_session(const std::shared_ptr<PortTunnelSession>& session) {
    {
        BasicLockGuard store_lock(mutex_);
        sessions_.erase(session->session_id);
    }

    std::vector<std::shared_ptr<RetainedTcpListener> > listeners;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_binds;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed) {
            return;
        }
        session->closed = true;
        session->expired = false;
        session->attached = false;
        session->resume_deadline_ms = 0ULL;
        session->connection.reset();
        for (std::map<uint32_t, std::shared_ptr<RetainedTcpListener> >::iterator it =
                 session->tcp_listeners.begin();
             it != session->tcp_listeners.end();
             ++it) {
            listeners.push_back(it->second);
        }
        session->tcp_listeners.clear();
        for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
                 session->udp_binds.begin();
             it != session->udp_binds.end();
             ++it) {
            udp_binds.push_back(it->second);
        }
        session->udp_binds.clear();
        session->state_changed.broadcast();
    }

    for (std::size_t i = 0; i < listeners.size(); ++i) {
        mark_retained_listener_closed(listeners[i]);
    }
    for (std::size_t i = 0; i < udp_binds.size(); ++i) {
        mark_udp_socket_closed(udp_binds[i]);
    }
}

void PortTunnelService::schedule_session_expiry(
    const std::shared_ptr<PortTunnelSession>& session
) {
#ifdef _WIN32
    struct ExpiryContext {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
    };

    struct ExpiryThread {
        static DWORD WINAPI entry(LPVOID raw_context) {
            std::unique_ptr<ExpiryContext> context(static_cast<ExpiryContext*>(raw_context));
            platform::sleep_ms(RESUME_TIMEOUT_MS);
            context->service->expire_session_if_needed(context->session);
            return 0;
        }
    };

    std::unique_ptr<ExpiryContext> context(new ExpiryContext());
    context->service = shared_from_this();
    context->session = session;
    HANDLE handle = CreateThread(NULL, 0, &ExpiryThread::entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    std::thread([service, session]() {
        platform::sleep_ms(RESUME_TIMEOUT_MS);
        service->expire_session_if_needed(session);
    }).detach();
#endif
}

void PortTunnelService::expire_session_if_needed(
    const std::shared_ptr<PortTunnelSession>& session
) {
    std::vector<std::shared_ptr<RetainedTcpListener> > listeners;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_binds;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired || session->attached) {
            return;
        }
        if (session->resume_deadline_ms == 0ULL ||
            platform::monotonic_ms() < session->resume_deadline_ms) {
            return;
        }
        session->expired = true;
        session->resume_deadline_ms = 0ULL;
        session->connection.reset();
        for (std::map<uint32_t, std::shared_ptr<RetainedTcpListener> >::iterator it =
                 session->tcp_listeners.begin();
             it != session->tcp_listeners.end();
             ++it) {
            listeners.push_back(it->second);
        }
        session->tcp_listeners.clear();
        for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
                 session->udp_binds.begin();
             it != session->udp_binds.end();
             ++it) {
            udp_binds.push_back(it->second);
        }
        session->udp_binds.clear();
        session->state_changed.broadcast();
    }

    for (std::size_t i = 0; i < listeners.size(); ++i) {
        mark_retained_listener_closed(listeners[i]);
    }
    for (std::size_t i = 0; i < udp_binds.size(); ++i) {
        mark_udp_socket_closed(udp_binds[i]);
    }
}

std::shared_ptr<PortTunnelConnection> PortTunnelService::wait_for_attachment(
    const std::shared_ptr<PortTunnelSession>& session
) {
    BasicLockGuard lock(session->mutex);
    for (;;) {
        if (session->closed || session->expired) {
            return std::shared_ptr<PortTunnelConnection>();
        }
        if (session->attached) {
            std::shared_ptr<PortTunnelConnection> connection = session->connection.lock();
            if (connection.get() != NULL) {
                return connection;
            }
        }
        session->state_changed.timed_wait_ms(session->mutex, RETAINED_SOCKET_POLL_TIMEOUT_MS);
    }
}

void PortTunnelService::spawn_tcp_listener_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<RetainedTcpListener>& listener
) {
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
        std::shared_ptr<RetainedTcpListener> listener;
    };

    struct ThreadEntry {
        static DWORD WINAPI entry(LPVOID raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->service->tcp_accept_loop(context->session, context->listener);
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = shared_from_this();
    context->session = session;
    context->listener = listener;
    HANDLE handle = CreateThread(NULL, 0, &ThreadEntry::entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    std::thread([service, session, listener]() {
        service->tcp_accept_loop(session, listener);
    }).detach();
#endif
}

void PortTunnelService::spawn_udp_bind_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelService> service;
        std::shared_ptr<PortTunnelSession> session;
        uint32_t stream_id;
        std::shared_ptr<TunnelUdpSocket> socket_value;
    };

    struct ThreadEntry {
        static DWORD WINAPI entry(LPVOID raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->service->udp_read_loop(
                context->session,
                context->stream_id,
                context->socket_value
            );
            return 0;
        }
    };

    std::unique_ptr<Context> context(new Context());
    context->service = shared_from_this();
    context->session = session;
    context->stream_id = stream_id;
    context->socket_value = socket_value;
    HANDLE handle = CreateThread(NULL, 0, &ThreadEntry::entry, context.get(), 0, NULL);
    if (handle != NULL) {
        context.release();
        CloseHandle(handle);
    }
#else
    std::shared_ptr<PortTunnelService> service = shared_from_this();
    std::thread([service, session, stream_id, socket_value]() {
        service->udp_read_loop(session, stream_id, socket_value);
    }).detach();
#endif
}

void PortTunnelService::tcp_accept_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    const std::shared_ptr<RetainedTcpListener>& listener
) {
    for (;;) {
        std::shared_ptr<PortTunnelConnection> connection = wait_for_attachment(session);
        if (connection.get() == NULL) {
            return;
        }

        const int ready =
            wait_socket_readable(listener->listener.get(), RETAINED_SOCKET_POLL_TIMEOUT_MS);
        if (ready == 0) {
            continue;
        }
        if (ready < 0) {
            if (retained_listener_closed(listener) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(
                    listener->stream_id,
                    "port_accept_failed",
                    socket_error_message("select")
                );
            }
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }

        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const SOCKET accepted = accept(
            listener->listener.get(),
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (accepted == INVALID_SOCKET) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                continue;
            }
            if (retained_listener_closed(listener) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(
                    listener->stream_id,
                    "port_accept_failed",
                    socket_error_message("accept")
                );
            }
            return;
        }
        UniqueSocket accepted_socket(accepted);
        if (!connection->owns_session(session)) {
            continue;
        }
        if (!connection->accept_session_tcp_stream(
                session,
                listener->stream_id,
                std::move(accepted_socket),
                printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)
            )) {
            if (session_is_unavailable(session)) {
                return;
            }
            continue;
        }
    }
}

void PortTunnelService::udp_read_loop(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id,
    const std::shared_ptr<TunnelUdpSocket>& socket_value
) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        std::shared_ptr<PortTunnelConnection> connection = wait_for_attachment(session);
        if (connection.get() == NULL) {
            return;
        }

        const int ready =
            wait_socket_readable(socket_value->socket.get(), RETAINED_SOCKET_POLL_TIMEOUT_MS);
        if (ready == 0) {
            continue;
        }
        if (ready < 0) {
            if (udp_socket_closed(socket_value) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(
                    stream_id,
                    "port_read_failed",
                    socket_error_message("select")
                );
            }
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }

        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const int received = recvfrom(
            socket_value->socket.get(),
            reinterpret_cast<char*>(buffer.data()),
            static_cast<int>(buffer.size()),
            0,
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (received < 0) {
            const int error = last_socket_error();
            if (receive_timeout_error(error)) {
                continue;
            }
            if (udp_socket_closed(socket_value) || session_is_unavailable(session)) {
                return;
            }
            if (connection->owns_session(session)) {
                connection->send_error(
                    stream_id,
                    "port_read_failed",
                    socket_error_message("recvfrom")
                );
            }
            return;
        }
        if (!connection->owns_session(session)) {
            continue;
        }
        std::vector<unsigned char> payload(buffer.begin(), buffer.begin() + received);
        if (!connection->emit_session_udp_datagram(
                session,
                stream_id,
                printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len),
                payload
            )) {
            if (session_is_unavailable(session)) {
                return;
            }
        }
    }
}

int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        HttpResponse response;
        write_bearer_auth_challenge(response);
        send_all(client, render_http_response(response));
        return response.status;
    }
    if (request.method != "POST" || request.path != "/v1/port/tunnel" ||
        !connection_header_has_upgrade(request) ||
        header_token_lower(request, "upgrade") != "remote-exec-port-tunnel" ||
        request.header("x-remote-exec-port-tunnel-version") != "2") {
        HttpResponse response;
        write_rpc_error(response, 400, "bad_request", "invalid port tunnel upgrade request");
        send_all(client, render_http_response(response));
        return response.status;
    }

    send_all(
        client,
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: remote-exec-port-tunnel\r\n"
        "\r\n"
    );
    if (!state.port_tunnel_service) {
        state.port_tunnel_service = create_port_tunnel_service();
    }
    std::shared_ptr<PortTunnelConnection> tunnel(
        new PortTunnelConnection(client, state.port_tunnel_service)
    );
    tunnel->run();
    return 101;
}

bool PortTunnelConnection::read_exact(unsigned char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int received = recv(
            client_,
            reinterpret_cast<char*>(data + offset),
            static_cast<int>(size - offset),
            0
        );
        if (received == 0) {
            return false;
        }
        if (received < 0) {
            return false;
        }
        offset += static_cast<std::size_t>(received);
    }
    return true;
}

bool PortTunnelConnection::read_preface() {
    std::vector<unsigned char> bytes(port_tunnel_preface_size(), 0U);
    if (!read_exact(bytes.data(), bytes.size())) {
        return false;
    }
    return std::string(reinterpret_cast<const char*>(bytes.data()), bytes.size()) ==
           std::string(port_tunnel_preface(), port_tunnel_preface_size());
}

bool PortTunnelConnection::read_frame(PortTunnelFrame* frame) {
    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    if (!read_exact(bytes.data(), bytes.size())) {
        return false;
    }
    const uint32_t meta_len = (static_cast<uint32_t>(bytes[8]) << 24) |
                              (static_cast<uint32_t>(bytes[9]) << 16) |
                              (static_cast<uint32_t>(bytes[10]) << 8) |
                              static_cast<uint32_t>(bytes[11]);
    const uint32_t data_len = (static_cast<uint32_t>(bytes[12]) << 24) |
                              (static_cast<uint32_t>(bytes[13]) << 16) |
                              (static_cast<uint32_t>(bytes[14]) << 8) |
                              static_cast<uint32_t>(bytes[15]);
    if (meta_len > PORT_TUNNEL_MAX_META_LEN || data_len > PORT_TUNNEL_MAX_DATA_LEN) {
        throw PortTunnelFrameError("port tunnel frame exceeds maximum length");
    }
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U &&
        !read_exact(bytes.data() + PORT_TUNNEL_HEADER_LEN, meta_len + data_len)) {
        return false;
    }
    *frame = decode_port_tunnel_frame(bytes);
    return true;
}

void PortTunnelConnection::send_frame(const PortTunnelFrame& frame) {
    const std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
    BasicLockGuard lock(writer_mutex_);
    if (closed_.load()) {
        return;
    }
    try {
        send_all_bytes(client_, reinterpret_cast<const char*>(bytes.data()), bytes.size());
    } catch (const std::exception&) {
        closed_.store(true);
        shutdown_socket(client_);
    }
}

void PortTunnelConnection::send_error(
    uint32_t stream_id,
    const std::string& code,
    const std::string& message
) {
    PortTunnelFrame frame;
    frame.type = PortTunnelFrameType::Error;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta = Json{{"code", code}, {"message", message}, {"fatal", false}}.dump();
    try {
        send_frame(frame);
    } catch (const std::exception&) {
    }
}

void PortTunnelConnection::run() {
    if (!read_preface()) {
        return;
    }

    try {
        for (;;) {
            PortTunnelFrame frame;
            if (!read_frame(&frame)) {
                break;
            }
            handle_frame(frame);
        }
    } catch (const std::exception& ex) {
        send_error(0U, "invalid_port_tunnel", ex.what());
    }
    detach_session();
    close_transport_owned_state();
}

void PortTunnelConnection::handle_frame(const PortTunnelFrame& frame) {
    try {
        switch (frame.type) {
        case PortTunnelFrameType::SessionOpen:
            session_open(frame);
            break;
        case PortTunnelFrameType::SessionResume:
            session_resume(frame);
            break;
        case PortTunnelFrameType::TcpListen:
            tcp_listen(frame);
            break;
        case PortTunnelFrameType::TcpConnect:
            tcp_connect(frame);
            break;
        case PortTunnelFrameType::TcpData:
            tcp_data(frame.stream_id, frame.data);
            break;
        case PortTunnelFrameType::TcpEof:
            tcp_eof(frame.stream_id);
            break;
        case PortTunnelFrameType::UdpBind:
            udp_bind(frame);
            break;
        case PortTunnelFrameType::UdpDatagram:
            udp_datagram(frame);
            break;
        case PortTunnelFrameType::Close:
            close_stream(frame.stream_id);
            break;
        default:
            send_error(frame.stream_id, "invalid_port_tunnel", "unexpected frame from broker");
            break;
        }
    } catch (const PortForwardError& ex) {
        send_error(frame.stream_id, ex.code(), ex.what());
    } catch (const std::exception& ex) {
        send_error(frame.stream_id, "internal_error", ex.what());
    }
}

void PortTunnelConnection::session_open(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "session open must use stream_id 0");
    }
    if (session_mode_active()) {
        throw PortForwardError(
            400,
            "port_tunnel_already_attached",
            "port tunnel session already attached"
        );
    }
    std::shared_ptr<PortTunnelSession> session = service_->create_session();
    {
        BasicLockGuard lock(state_mutex_);
        session_ = session;
    }
    service_->attach_session(session, shared_from_this());

    PortTunnelFrame ready = make_empty_frame(PortTunnelFrameType::SessionReady, 0U);
    ready.meta = Json{
        {"session_id", session->session_id},
        {"resume_timeout_ms", RESUME_TIMEOUT_MS}
    }.dump();
    send_frame(ready);
}

void PortTunnelConnection::session_resume(const PortTunnelFrame& frame) {
    if (frame.stream_id != 0U) {
        throw PortForwardError(400, "invalid_port_tunnel", "session resume must use stream_id 0");
    }
    if (session_mode_active()) {
        throw PortForwardError(
            400,
            "port_tunnel_already_attached",
            "port tunnel session already attached"
        );
    }

    const std::string session_id = frame_meta_string(frame, "session_id");
    std::shared_ptr<PortTunnelSession> session = service_->find_session(session_id);
    if (session.get() == NULL) {
        throw PortForwardError(
            400,
            "unknown_port_tunnel_session",
            "unknown port tunnel session"
        );
    }

    bool expired = false;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed) {
            throw PortForwardError(
                400,
                "unknown_port_tunnel_session",
                "unknown port tunnel session"
            );
        }
        if (session->attached) {
            throw PortForwardError(
                400,
                "port_tunnel_already_attached",
                "port tunnel session is already attached"
            );
        }
        expired = session->expired ||
                  (session->resume_deadline_ms != 0ULL &&
                   platform::monotonic_ms() >= session->resume_deadline_ms);
    }
    if (expired) {
        service_->close_session(session);
        throw PortForwardError(
            400,
            "port_tunnel_resume_expired",
            "port tunnel resume expired"
        );
    }

    {
        BasicLockGuard lock(state_mutex_);
        session_ = session;
    }
    service_->attach_session(session, shared_from_this());
    send_frame(make_empty_frame(PortTunnelFrameType::SessionResumed, 0U));
}

void PortTunnelConnection::tcp_listen(const PortTunnelFrame& frame) {
    const std::string endpoint = normalize_port_forward_endpoint(frame_meta_string(frame, "endpoint"));
    std::string bound_endpoint;

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        std::shared_ptr<RetainedTcpListener> listener(
            new RetainedTcpListener(frame.stream_id, bind_port_forward_socket(endpoint, "tcp"))
        );
        bound_endpoint = socket_local_endpoint(listener->listener.get());
        {
            BasicLockGuard lock(session->mutex);
            session->tcp_listeners[frame.stream_id] = listener;
        }
        service_->spawn_tcp_listener_loop(session, listener);
    } else {
        UniqueSocket listener(bind_port_forward_socket(endpoint, "tcp"));
        const SOCKET listener_socket = listener.get();
        bound_endpoint = socket_local_endpoint(listener_socket);
        {
            BasicLockGuard lock(state_mutex_);
            tcp_listeners_[frame.stream_id].reset(listener.release());
        }
        spawn_tcp_accept_thread(shared_from_this(), frame.stream_id, listener_socket);
    }

    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::TcpListenOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
}

void PortTunnelConnection::tcp_accept_loop_transport_owned(
    uint32_t listener_stream_id,
    SOCKET listener_socket
) {
    for (;;) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const SOCKET accepted = accept(
            listener_socket,
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (accepted == INVALID_SOCKET) {
            return;
        }
        const uint32_t stream_id = next_daemon_stream_id_.fetch_add(2U);
        std::shared_ptr<TunnelTcpStream> stream(new TunnelTcpStream(accepted));
        {
            BasicLockGuard lock(state_mutex_);
            if (closed_.load()) {
                mark_tcp_stream_closed(stream);
                return;
            }
            tcp_streams_[stream_id] = stream;
        }
        PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpAccept, stream_id);
        frame.meta = Json{
            {"listener_stream_id", listener_stream_id},
            {"peer", printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)}
        }.dump();
        send_frame(frame);
        spawn_tcp_read_thread(shared_from_this(), stream_id, stream);
    }
}

void PortTunnelConnection::tcp_connect(const PortTunnelFrame& frame) {
    const std::string endpoint = ensure_nonzero_connect_endpoint(frame_meta_string(frame, "endpoint"));
    std::shared_ptr<TunnelTcpStream> stream(
        new TunnelTcpStream(connect_port_forward_socket(endpoint, "tcp"))
    );
    {
        BasicLockGuard lock(state_mutex_);
        tcp_streams_[frame.stream_id] = stream;
    }
    send_frame(make_empty_frame(PortTunnelFrameType::TcpConnectOk, frame.stream_id));
    spawn_tcp_read_thread(shared_from_this(), frame.stream_id, stream);
}

void PortTunnelConnection::tcp_read_loop(
    uint32_t stream_id,
    std::shared_ptr<TunnelTcpStream> stream
) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        const int received = recv(
            stream->socket.get(),
            reinterpret_cast<char*>(buffer.data()),
            static_cast<int>(buffer.size()),
            0
        );
        if (received == 0) {
            send_frame(make_empty_frame(PortTunnelFrameType::TcpEof, stream_id));
            return;
        }
        if (received < 0) {
            if (!tcp_stream_closed(stream)) {
                send_error(stream_id, "port_read_failed", socket_error_message("recv"));
            }
            return;
        }
        PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpData, stream_id);
        frame.data.assign(buffer.begin(), buffer.begin() + received);
        send_frame(frame);
    }
}

void PortTunnelConnection::tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data) {
    std::shared_ptr<TunnelTcpStream> stream;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
            tcp_streams_.find(stream_id);
        if (it == tcp_streams_.end()) {
            throw PortForwardError(400, "unknown_port_connection", "unknown tunnel tcp stream");
        }
        stream = it->second;
    }
    BasicLockGuard lock(stream->mutex);
    if (stream->closed) {
        throw PortForwardError(400, "port_connection_closed", "connection was closed");
    }
    send_all_bytes(
        stream->socket.get(),
        reinterpret_cast<const char*>(data.data()),
        data.size()
    );
}

void PortTunnelConnection::tcp_eof(uint32_t stream_id) {
    std::shared_ptr<TunnelTcpStream> stream;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
            tcp_streams_.find(stream_id);
        if (it == tcp_streams_.end()) {
            return;
        }
        stream = it->second;
    }
#ifdef _WIN32
    shutdown(stream->socket.get(), SD_SEND);
#else
    shutdown(stream->socket.get(), SHUT_WR);
#endif
}

void PortTunnelConnection::udp_bind(const PortTunnelFrame& frame) {
    const std::string endpoint = normalize_port_forward_endpoint(frame_meta_string(frame, "endpoint"));
    std::shared_ptr<TunnelUdpSocket> socket_value(
        new TunnelUdpSocket(bind_port_forward_socket(endpoint, "udp"))
    );
    const std::string bound_endpoint = socket_local_endpoint(socket_value->socket.get());

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        {
            BasicLockGuard lock(session->mutex);
            session->udp_binds[frame.stream_id] = socket_value;
        }
        service_->spawn_udp_bind_loop(session, frame.stream_id, socket_value);
    } else {
        {
            BasicLockGuard lock(state_mutex_);
            udp_sockets_[frame.stream_id] = socket_value;
        }
        spawn_udp_read_thread(shared_from_this(), frame.stream_id, socket_value);
    }

    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::UdpBindOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
}

void PortTunnelConnection::udp_read_loop_transport_owned(
    uint32_t stream_id,
    std::shared_ptr<TunnelUdpSocket> socket_value
) {
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    for (;;) {
        sockaddr_storage peer_address;
        std::memset(&peer_address, 0, sizeof(peer_address));
        socklen_t peer_len = sizeof(peer_address);
        const int received = recvfrom(
            socket_value->socket.get(),
            reinterpret_cast<char*>(buffer.data()),
            static_cast<int>(buffer.size()),
            0,
            reinterpret_cast<sockaddr*>(&peer_address),
            &peer_len
        );
        if (received < 0) {
            if (!udp_socket_closed(socket_value)) {
                send_error(stream_id, "port_read_failed", socket_error_message("recvfrom"));
            }
            return;
        }
        PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::UdpDatagram, stream_id);
        frame.meta = Json{
            {"peer", printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)}
        }.dump();
        frame.data.assign(buffer.begin(), buffer.begin() + received);
        send_frame(frame);
    }
}

void PortTunnelConnection::udp_datagram(const PortTunnelFrame& frame) {
    std::shared_ptr<TunnelUdpSocket> socket_value;
    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        BasicLockGuard lock(session->mutex);
        std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
            session->udp_binds.find(frame.stream_id);
        if (it == session->udp_binds.end()) {
            throw PortForwardError(400, "unknown_port_bind", "unknown tunnel udp stream");
        }
        socket_value = it->second;
    } else {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
            udp_sockets_.find(frame.stream_id);
        if (it == udp_sockets_.end()) {
            throw PortForwardError(400, "unknown_port_bind", "unknown tunnel udp stream");
        }
        socket_value = it->second;
    }
    const std::string peer = frame_meta_string(frame, "peer");
    socklen_t peer_len = 0;
    const sockaddr_storage peer_address = parse_port_forward_peer(peer, &peer_len);
    const int sent = sendto(
        socket_value->socket.get(),
        reinterpret_cast<const char*>(frame.data.data()),
        static_cast<int>(frame.data.size()),
        0,
        reinterpret_cast<const sockaddr*>(&peer_address),
        peer_len
    );
    if (sent < 0 || static_cast<std::size_t>(sent) != frame.data.size()) {
        throw PortForwardError(400, "port_write_failed", socket_error_message("sendto"));
    }
}

void PortTunnelConnection::close_stream(uint32_t stream_id) {
    std::shared_ptr<TunnelTcpStream> tcp_stream;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator tcp =
            tcp_streams_.find(stream_id);
        if (tcp != tcp_streams_.end()) {
            tcp_stream = tcp->second;
            tcp_streams_.erase(tcp);
        }
    }
    if (tcp_stream.get() != NULL) {
        mark_tcp_stream_closed(tcp_stream);
    }

    if (session_mode_active()) {
        std::shared_ptr<PortTunnelSession> session = current_session();
        bool close_session_now = false;
        {
            BasicLockGuard lock(session->mutex);
            std::map<uint32_t, std::shared_ptr<RetainedTcpListener> >::iterator listener =
                session->tcp_listeners.find(stream_id);
            if (listener != session->tcp_listeners.end()) {
                mark_retained_listener_closed(listener->second);
                session->tcp_listeners.erase(listener);
                close_session_now = true;
            }
            std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator udp =
                session->udp_binds.find(stream_id);
            if (udp != session->udp_binds.end()) {
                mark_udp_socket_closed(udp->second);
                session->udp_binds.erase(udp);
                close_session_now = true;
            }
        }
        if (close_session_now) {
            service_->close_session(session);
        }
        return;
    }

    std::shared_ptr<TunnelUdpSocket> udp_socket;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, UniqueSocket>::iterator listener = tcp_listeners_.find(stream_id);
        if (listener != tcp_listeners_.end()) {
            shutdown_socket(listener->second.get());
            listener->second.reset();
            tcp_listeners_.erase(listener);
        }
        std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator udp =
            udp_sockets_.find(stream_id);
        if (udp != udp_sockets_.end()) {
            udp_socket = udp->second;
            udp_sockets_.erase(udp);
        }
    }
    if (udp_socket.get() != NULL) {
        mark_udp_socket_closed(udp_socket);
    }
}

void PortTunnelConnection::detach_session() {
    std::shared_ptr<PortTunnelSession> session = current_session();
    if (session.get() != NULL) {
        service_->detach_session(session);
    }
}

void PortTunnelConnection::close_transport_owned_state() {
    std::map<uint32_t, UniqueSocket> tcp_listeners;
    std::vector<std::shared_ptr<TunnelTcpStream> > tcp_streams;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_sockets;
    bool session_mode = false;
    {
        BasicLockGuard lock(state_mutex_);
        closed_.store(true);
        session_mode = session_.get() != NULL;
        if (!session_mode) {
            tcp_listeners.swap(tcp_listeners_);
        }
        for (std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
                 tcp_streams_.begin();
             it != tcp_streams_.end();
             ++it) {
            tcp_streams.push_back(it->second);
        }
        tcp_streams_.clear();
        for (std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator it =
                 udp_sockets_.begin();
             it != udp_sockets_.end();
             ++it) {
            udp_sockets.push_back(it->second);
        }
        udp_sockets_.clear();
    }
    for (std::map<uint32_t, UniqueSocket>::iterator it = tcp_listeners.begin();
         it != tcp_listeners.end();
         ++it) {
        shutdown_socket(it->second.get());
        it->second.reset();
    }
    for (std::size_t i = 0; i < tcp_streams.size(); ++i) {
        mark_tcp_stream_closed(tcp_streams[i]);
    }
    for (std::size_t i = 0; i < udp_sockets.size(); ++i) {
        mark_udp_socket_closed(udp_sockets[i]);
    }
}

std::shared_ptr<PortTunnelSession> PortTunnelConnection::current_session() {
    BasicLockGuard lock(state_mutex_);
    return session_;
}

bool PortTunnelConnection::session_mode_active() {
    return current_session().get() != NULL;
}

bool PortTunnelConnection::owns_session(const std::shared_ptr<PortTunnelSession>& session) {
    BasicLockGuard lock(state_mutex_);
    return !closed_.load() && session_.get() == session.get();
}

bool PortTunnelConnection::accept_session_tcp_stream(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t listener_stream_id,
    UniqueSocket accepted_socket,
    const std::string& peer
) {
    uint32_t stream_id = 0U;
    {
        BasicLockGuard lock(session->mutex);
        if (session->closed || session->expired) {
            return false;
        }
        stream_id = session->next_daemon_stream_id;
        session->next_daemon_stream_id += 2U;
    }

    std::shared_ptr<TunnelTcpStream> stream(new TunnelTcpStream(accepted_socket.release()));
    {
        BasicLockGuard lock(state_mutex_);
        if (closed_.load() || session_.get() != session.get()) {
            mark_tcp_stream_closed(stream);
            return false;
        }
        tcp_streams_[stream_id] = stream;
    }

    PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::TcpAccept, stream_id);
    frame.meta = Json{
        {"listener_stream_id", listener_stream_id},
        {"peer", peer}
    }.dump();
    send_frame(frame);
    if (!owns_session(session) || closed_.load()) {
        std::shared_ptr<TunnelTcpStream> removed_stream;
        {
            BasicLockGuard lock(state_mutex_);
            std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator it =
                tcp_streams_.find(stream_id);
            if (it != tcp_streams_.end()) {
                removed_stream = it->second;
                tcp_streams_.erase(it);
            }
        }
        if (removed_stream.get() != NULL) {
            mark_tcp_stream_closed(removed_stream);
        }
        return false;
    }

    spawn_tcp_read_thread(shared_from_this(), stream_id, stream);
    return true;
}

bool PortTunnelConnection::emit_session_udp_datagram(
    const std::shared_ptr<PortTunnelSession>& session,
    uint32_t stream_id,
    const std::string& peer,
    const std::vector<unsigned char>& data
) {
    if (!owns_session(session)) {
        return false;
    }
    PortTunnelFrame frame = make_empty_frame(PortTunnelFrameType::UdpDatagram, stream_id);
    frame.meta = Json{{"peer", peer}}.dump();
    frame.data = data;
    send_frame(frame);
    return owns_session(session) && !closed_.load();
}
