#include "port_forward.h"

#include <atomic>
#include <cstring>
#include <sstream>
#include <vector>

#include "logging.h"
#include "port_forward_codec.h"
#include "port_forward_endpoint.h"
#include "port_forward_socket_ops.h"

namespace {

const std::size_t READ_BUF_SIZE = 64U * 1024U;

std::atomic<unsigned long> next_port_id(1UL);

std::string make_port_id(const char* prefix) {
    std::ostringstream out;
    out << prefix << '_' << next_port_id.fetch_add(1UL);
    return out.str();
}

PortForwardError bind_closed_error(const std::string& bind_id) {
    return PortForwardError(400, "port_bind_closed", "bind `" + bind_id + "` was closed");
}

PortForwardError connection_closed_error(const std::string& connection_id) {
    return PortForwardError(
        400,
        "port_connection_closed",
        "connection `" + connection_id + "` was closed"
    );
}

bool shared_socket_closed(const std::shared_ptr<SharedSocket>& socket_value) {
    BasicLockGuard lock(socket_value->state_mutex);
    return socket_value->closed;
}

bool tcp_connection_closed(const std::shared_ptr<TcpConnection>& connection) {
    BasicLockGuard lock(connection->state_mutex);
    return connection->closed;
}

void mark_shared_socket_closed(const std::shared_ptr<SharedSocket>& socket_value) {
    BasicLockGuard lock(socket_value->state_mutex);
    if (!socket_value->closed) {
        socket_value->closed = true;
        shutdown_socket(socket_value->socket.get());
        socket_value->socket.reset();
    }
}

void mark_tcp_connection_closed(const std::shared_ptr<TcpConnection>& connection) {
    BasicLockGuard lock(connection->state_mutex);
    if (!connection->closed) {
        connection->closed = true;
        shutdown_socket(connection->socket.get());
        connection->socket.reset();
    }
}

}  // namespace

PortForwardError::PortForwardError(
    int status,
    const std::string& code,
    const std::string& message
)
    : std::runtime_error(message), status_(status), code_(code) {}

int PortForwardError::status() const {
    return status_;
}

const std::string& PortForwardError::code() const {
    return code_;
}

TcpConnection::TcpConnection(SOCKET socket_value) : socket(socket_value), closed(false) {}

SharedSocket::SharedSocket(SOCKET socket_value) : socket(socket_value), closed(false) {}

PortForwardStore::PortForwardStore() {}

PortForwardStore::~PortForwardStore() {}

Json PortForwardStore::listen(const std::string& endpoint, const std::string& protocol) {
    const std::string normalized = normalize_port_forward_endpoint(endpoint);
    const SOCKET socket_value = bind_port_forward_socket(normalized, protocol);
    UniqueSocket socket(socket_value);
    const std::string bound_endpoint = socket_local_endpoint(socket.get());
    const std::string bind_id = make_port_id("bind");

    {
        BasicLockGuard lock(mutex_);
        if (protocol == "tcp") {
            tcp_listeners_[bind_id] = std::shared_ptr<SharedSocket>(
                new SharedSocket(socket.release())
            );
        } else if (protocol == "udp") {
            udp_sockets_[bind_id] = std::shared_ptr<SharedSocket>(
                new SharedSocket(socket.release())
            );
        } else {
            throw PortForwardError(
                400,
                "bad_request",
                "unsupported port forward protocol `" + protocol + "`"
            );
        }
    }

    log_message(
        LOG_INFO,
        "port_forward",
        "opened listener bind_id=`" + bind_id + "` endpoint=`" + bound_endpoint +
            "` protocol=`" + protocol + "`"
    );
    return Json{{"bind_id", bind_id}, {"endpoint", bound_endpoint}};
}

Json PortForwardStore::listen_accept(const std::string& bind_id) {
    const std::shared_ptr<SharedSocket> listener = tcp_listener(bind_id);

    sockaddr_storage peer_address;
    std::memset(&peer_address, 0, sizeof(peer_address));
    socklen_t peer_len = sizeof(peer_address);
    const SOCKET accepted = accept(
        listener->socket.get(),
        reinterpret_cast<sockaddr*>(&peer_address),
        &peer_len
    );
    if (accepted == INVALID_SOCKET) {
        if (shared_socket_closed(listener)) {
            throw bind_closed_error(bind_id);
        }
        throw PortForwardError(400, "port_accept_failed", socket_error_message("accept"));
    }
    if (shared_socket_closed(listener)) {
        UniqueSocket cleanup(accepted);
        throw bind_closed_error(bind_id);
    }

    const std::string connection_id = make_port_id("conn");
    {
        BasicLockGuard lock(mutex_);
        tcp_connections_[connection_id] = std::shared_ptr<TcpConnection>(
            new TcpConnection(accepted)
        );
    }

    log_message(
        LOG_DEBUG,
        "port_forward",
        "accepted tcp connection bind_id=`" + bind_id + "` connection_id=`" +
            connection_id + "` peer=`" +
            printable_port_forward_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len) +
            "`"
    );
    return Json{{"connection_id", connection_id}};
}

Json PortForwardStore::listen_close(const std::string& bind_id) {
    std::shared_ptr<SharedSocket> listener;
    std::shared_ptr<SharedSocket> socket_value;
    {
        BasicLockGuard lock(mutex_);
        std::map<std::string, std::shared_ptr<SharedSocket> >::iterator tcp_it =
            tcp_listeners_.find(bind_id);
        if (tcp_it != tcp_listeners_.end()) {
            listener = tcp_it->second;
            tcp_listeners_.erase(tcp_it);
        }
        std::map<std::string, std::shared_ptr<SharedSocket> >::iterator udp_it =
            udp_sockets_.find(bind_id);
        if (udp_it != udp_sockets_.end()) {
            socket_value = udp_it->second;
            udp_sockets_.erase(udp_it);
        }
    }
    if (listener.get() != NULL) {
        mark_shared_socket_closed(listener);
    }
    if (socket_value.get() != NULL) {
        mark_shared_socket_closed(socket_value);
    }
    log_message(LOG_DEBUG, "port_forward", "closed bind `" + bind_id + "`");
    return Json::object();
}

Json PortForwardStore::connect(const std::string& endpoint, const std::string& protocol) {
    if (protocol == "udp") {
        throw PortForwardError(
            400,
            "unsupported_operation",
            "udp connect is not used by this forwarding protocol"
        );
    }
    if (protocol != "tcp") {
        throw PortForwardError(
            400,
            "bad_request",
            "unsupported port forward protocol `" + protocol + "`"
        );
    }

    const std::string normalized = ensure_nonzero_connect_endpoint(endpoint);
    const SOCKET socket_value = connect_port_forward_socket(normalized, protocol);
    const std::string connection_id = make_port_id("conn");
    {
        BasicLockGuard lock(mutex_);
        tcp_connections_[connection_id] = std::shared_ptr<TcpConnection>(
            new TcpConnection(socket_value)
        );
    }
    log_message(
        LOG_DEBUG,
        "port_forward",
        "opened tcp connection connection_id=`" + connection_id + "` endpoint=`" +
            normalized + "`"
    );
    return Json{{"connection_id", connection_id}};
}

Json PortForwardStore::connection_read(const std::string& connection_id) {
    const std::shared_ptr<TcpConnection> connection = tcp_connection(connection_id);
    BasicLockGuard read_lock(connection->read_mutex);
    if (tcp_connection_closed(connection)) {
        throw connection_closed_error(connection_id);
    }

    std::string buffer;
    buffer.resize(READ_BUF_SIZE);
    const int received = recv(
        connection->socket.get(),
        &buffer[0],
        static_cast<int>(buffer.size()),
        0
    );
    if (received < 0) {
        if (tcp_connection_closed(connection)) {
            throw connection_closed_error(connection_id);
        }
        throw PortForwardError(400, "port_read_failed", socket_error_message("recv"));
    }
    if (tcp_connection_closed(connection)) {
        throw connection_closed_error(connection_id);
    }
    if (received == 0) {
        connection_close(connection_id);
        return Json{{"data", ""}, {"eof", true}};
    }
    buffer.resize(static_cast<std::size_t>(received));
    return Json{{"data", base64_encode_bytes(buffer)}, {"eof", false}};
}

Json PortForwardStore::connection_write(
    const std::string& connection_id,
    const std::string& data
) {
    const std::shared_ptr<TcpConnection> connection = tcp_connection(connection_id);
    const std::string bytes = base64_decode_bytes(data);
    BasicLockGuard write_lock(connection->write_mutex);
    if (tcp_connection_closed(connection)) {
        throw connection_closed_error(connection_id);
    }
    try {
        send_all_socket(connection->socket.get(), bytes);
    } catch (const PortForwardError&) {
        if (tcp_connection_closed(connection)) {
            throw connection_closed_error(connection_id);
        }
        throw;
    }
    return Json::object();
}

Json PortForwardStore::connection_close(const std::string& connection_id) {
    std::shared_ptr<TcpConnection> connection;
    {
        BasicLockGuard lock(mutex_);
        std::map<std::string, std::shared_ptr<TcpConnection> >::iterator it =
            tcp_connections_.find(connection_id);
        if (it != tcp_connections_.end()) {
            connection = it->second;
            tcp_connections_.erase(it);
        }
    }
    if (connection.get() != NULL) {
        mark_tcp_connection_closed(connection);
    }
    log_message(LOG_DEBUG, "port_forward", "closed tcp connection `" + connection_id + "`");
    return Json::object();
}

Json PortForwardStore::udp_datagram_read(const std::string& bind_id) {
    const std::shared_ptr<SharedSocket> socket_value = udp_socket(bind_id);
    if (shared_socket_closed(socket_value)) {
        throw bind_closed_error(bind_id);
    }
    std::vector<unsigned char> buffer(READ_BUF_SIZE);
    sockaddr_storage peer_address;
    std::memset(&peer_address, 0, sizeof(peer_address));
    socklen_t peer_len = sizeof(peer_address);
    const int received = recvfrom(
        socket_value->socket.get(),
        reinterpret_cast<char*>(&buffer[0]),
        static_cast<int>(buffer.size()),
        0,
        reinterpret_cast<sockaddr*>(&peer_address),
        &peer_len
    );
    if (received < 0) {
        if (shared_socket_closed(socket_value)) {
            throw bind_closed_error(bind_id);
        }
        throw PortForwardError(400, "port_read_failed", socket_error_message("recvfrom"));
    }
    if (shared_socket_closed(socket_value)) {
        throw bind_closed_error(bind_id);
    }
    buffer.resize(static_cast<std::size_t>(received));
    const std::string peer = printable_port_forward_endpoint(
        reinterpret_cast<sockaddr*>(&peer_address),
        peer_len
    );
    const std::string payload(
        reinterpret_cast<const char*>(buffer.data()),
        buffer.size()
    );
    return Json{
        {"peer", peer},
        {"data", base64_encode_bytes(payload)},
    };
}

Json PortForwardStore::udp_datagram_write(
    const std::string& bind_id,
    const std::string& peer,
    const std::string& data
) {
    const std::shared_ptr<SharedSocket> socket_value = udp_socket(bind_id);
    const std::string bytes = base64_decode_bytes(data);
    if (shared_socket_closed(socket_value)) {
        throw bind_closed_error(bind_id);
    }
    socklen_t peer_len = 0;
    const sockaddr_storage peer_address = parse_port_forward_peer(peer, &peer_len);
    const int sent = sendto(
        socket_value->socket.get(),
        bytes.data(),
        static_cast<int>(bytes.size()),
        0,
        reinterpret_cast<const sockaddr*>(&peer_address),
        peer_len
    );
    if (sent < 0 || static_cast<std::size_t>(sent) != bytes.size()) {
        if (shared_socket_closed(socket_value)) {
            throw bind_closed_error(bind_id);
        }
        throw PortForwardError(400, "port_write_failed", socket_error_message("sendto"));
    }
    return Json::object();
}

std::shared_ptr<TcpConnection> PortForwardStore::tcp_connection(
    const std::string& connection_id
) {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<TcpConnection> >::iterator it =
        tcp_connections_.find(connection_id);
    if (it == tcp_connections_.end()) {
        throw PortForwardError(
            400,
            "unknown_port_connection",
            "unknown connection `" + connection_id + "`"
        );
    }
    return it->second;
}

std::shared_ptr<SharedSocket> PortForwardStore::tcp_listener(const std::string& bind_id) {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<SharedSocket> >::iterator it =
        tcp_listeners_.find(bind_id);
    if (it == tcp_listeners_.end()) {
        throw PortForwardError(400, "unknown_port_bind", "unknown bind `" + bind_id + "`");
    }
    return it->second;
}

std::shared_ptr<SharedSocket> PortForwardStore::udp_socket(const std::string& bind_id) {
    BasicLockGuard lock(mutex_);
    std::map<std::string, std::shared_ptr<SharedSocket> >::iterator it = udp_sockets_.find(bind_id);
    if (it == udp_sockets_.end()) {
        throw PortForwardError(400, "unknown_port_bind", "unknown bind `" + bind_id + "`");
    }
    return it->second;
}
