#include "port_tunnel.h"

#include <atomic>
#include <cstring>
#include <map>
#include <memory>
#include <sstream>
#include <string>
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
#include "port_forward_endpoint.h"
#include "port_forward_socket_ops.h"
#include "port_tunnel_frame.h"
#include "server_transport.h"
#include "text_utils.h"

using Json = nlohmann::json;

namespace {

const std::size_t READ_BUF_SIZE = 64U * 1024U;

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

class PortTunnelConnection : public std::enable_shared_from_this<PortTunnelConnection> {
public:
    explicit PortTunnelConnection(SOCKET client)
        : client_(client), closed_(false), next_daemon_stream_id_(2U) {}

    void run();
    void tcp_accept_loop(uint32_t listener_stream_id, SOCKET listener_socket);
    void tcp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelTcpStream> stream);
    void udp_read_loop(uint32_t stream_id, std::shared_ptr<TunnelUdpSocket> socket_value);

private:
    PortTunnelConnection(const PortTunnelConnection&) = delete;
    PortTunnelConnection& operator=(const PortTunnelConnection&) = delete;

    bool read_exact(unsigned char* data, std::size_t size);
    bool read_preface();
    bool read_frame(PortTunnelFrame* frame);
    void send_frame(const PortTunnelFrame& frame);
    void send_error(uint32_t stream_id, const std::string& code, const std::string& message);
    void handle_frame(const PortTunnelFrame& frame);
    void tcp_listen(const PortTunnelFrame& frame);
    void tcp_connect(const PortTunnelFrame& frame);
    void tcp_data(uint32_t stream_id, const std::vector<unsigned char>& data);
    void tcp_eof(uint32_t stream_id);
    void udp_bind(const PortTunnelFrame& frame);
    void udp_datagram(const PortTunnelFrame& frame);
    void close_stream(uint32_t stream_id);
    void close_all();

    SOCKET client_;
    BasicMutex writer_mutex_;
    BasicMutex state_mutex_;
    std::atomic<bool> closed_;
    std::map<uint32_t, UniqueSocket> tcp_listeners_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream> > tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_sockets_;
    std::atomic<uint32_t> next_daemon_stream_id_;
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
    tunnel->tcp_accept_loop(context->stream_id, context->socket);
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
    std::thread([tunnel, stream_id, socket]() { tunnel->tcp_accept_loop(stream_id, socket); }).detach();
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
    tunnel->udp_read_loop(context->stream_id, socket_value);
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
        tunnel->udp_read_loop(stream_id, socket_value);
    }).detach();
#endif
}

}  // namespace

bool is_port_tunnel_upgrade_request(const HttpRequest& request) {
    return request.method == "POST" && request.path == "/v1/port/tunnel";
}

int handle_port_tunnel_upgrade(AppState& state, SOCKET client, const HttpRequest& request) {
    (void)state;
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
        request.header("x-remote-exec-port-tunnel-version") != "1") {
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
    std::shared_ptr<PortTunnelConnection> tunnel(new PortTunnelConnection(client));
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
    close_all();
}

void PortTunnelConnection::handle_frame(const PortTunnelFrame& frame) {
    try {
        switch (frame.type) {
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

void PortTunnelConnection::tcp_listen(const PortTunnelFrame& frame) {
    const std::string endpoint = normalize_port_forward_endpoint(frame_meta_string(frame, "endpoint"));
    UniqueSocket listener(bind_port_forward_socket(endpoint, "tcp"));
    const SOCKET listener_socket = listener.get();
    const std::string bound_endpoint = socket_local_endpoint(listener_socket);
    {
        BasicLockGuard lock(state_mutex_);
        tcp_listeners_[frame.stream_id].reset(listener.release());
    }

    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::TcpListenOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
    spawn_tcp_accept_thread(shared_from_this(), frame.stream_id, listener_socket);
}

void PortTunnelConnection::tcp_accept_loop(uint32_t listener_stream_id, SOCKET listener_socket) {
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
    {
        BasicLockGuard lock(state_mutex_);
        udp_sockets_[frame.stream_id] = socket_value;
    }
    PortTunnelFrame ok = make_empty_frame(PortTunnelFrameType::UdpBindOk, frame.stream_id);
    ok.meta = Json{{"endpoint", bound_endpoint}}.dump();
    send_frame(ok);
    spawn_udp_read_thread(shared_from_this(), frame.stream_id, socket_value);
}

void PortTunnelConnection::udp_read_loop(
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
    {
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
    std::shared_ptr<TunnelUdpSocket> udp_socket;
    {
        BasicLockGuard lock(state_mutex_);
        std::map<uint32_t, UniqueSocket>::iterator listener = tcp_listeners_.find(stream_id);
        if (listener != tcp_listeners_.end()) {
            shutdown_socket(listener->second.get());
            listener->second.reset();
            tcp_listeners_.erase(listener);
        }
        std::map<uint32_t, std::shared_ptr<TunnelTcpStream> >::iterator tcp =
            tcp_streams_.find(stream_id);
        if (tcp != tcp_streams_.end()) {
            tcp_stream = tcp->second;
            tcp_streams_.erase(tcp);
        }
        std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> >::iterator udp =
            udp_sockets_.find(stream_id);
        if (udp != udp_sockets_.end()) {
            udp_socket = udp->second;
            udp_sockets_.erase(udp);
        }
    }
    if (tcp_stream.get() != NULL) {
        mark_tcp_stream_closed(tcp_stream);
    }
    if (udp_socket.get() != NULL) {
        mark_udp_socket_closed(udp_socket);
    }
}

void PortTunnelConnection::close_all() {
    std::map<uint32_t, UniqueSocket> tcp_listeners;
    std::vector<std::shared_ptr<TunnelTcpStream> > tcp_streams;
    std::vector<std::shared_ptr<TunnelUdpSocket> > udp_sockets;
    {
        BasicLockGuard lock(state_mutex_);
        closed_.store(true);
        tcp_listeners.swap(tcp_listeners_);
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
