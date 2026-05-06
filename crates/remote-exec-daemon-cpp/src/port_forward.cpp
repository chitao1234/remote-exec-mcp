#include "port_forward.h"

#include <atomic>
#include <cstring>
#include <sstream>
#include <vector>

#include "logging.h"
#include "port_forward_codec.h"
#include "port_forward_endpoint.h"
#include "port_forward_socket_ops.h"
#include "platform.h"

namespace {

const std::size_t READ_BUF_SIZE = 64U * 1024U;
const std::uint64_t MIN_LEASE_TTL_MS = 250U;

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
    return socket_value->state != PORT_RESOURCE_OPEN;
}

bool tcp_connection_closed(const std::shared_ptr<TcpConnection>& connection) {
    BasicLockGuard lock(connection->state_mutex);
    return connection->state != PORT_RESOURCE_OPEN;
}

void finish_close_shared_socket(const std::shared_ptr<SharedSocket>& socket_value) {
    BasicLockGuard lock(socket_value->state_mutex);
    if (socket_value->state == PORT_RESOURCE_CLOSED) {
        return;
    }
    socket_value->state = PORT_RESOURCE_CLOSING;
    shutdown_socket(socket_value->socket.get());
    socket_value->socket.reset();
    socket_value->state = PORT_RESOURCE_CLOSED;
}

void finish_close_tcp_connection(const std::shared_ptr<TcpConnection>& connection) {
    BasicLockGuard lock(connection->state_mutex);
    if (connection->state == PORT_RESOURCE_CLOSED) {
        return;
    }
    connection->state = PORT_RESOURCE_CLOSING;
    shutdown_socket(connection->socket.get());
    connection->socket.reset();
    connection->state = PORT_RESOURCE_CLOSED;
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

TcpConnection::TcpConnection(SOCKET socket_value, const std::string& lease)
    : socket(socket_value), state(PORT_RESOURCE_OPEN), lease_id(lease) {}

SharedSocket::SharedSocket(SOCKET socket_value, const std::string& lease)
    : socket(socket_value), state(PORT_RESOURCE_OPEN), lease_id(lease) {}

PortForwardStore::PortForwardStore() {}

PortForwardStore::~PortForwardStore() {}

Json PortForwardStore::listen(
    const std::string& endpoint,
    const std::string& protocol,
    const std::string& lease_id,
    std::uint64_t lease_ttl_ms
) {
    sweep_expired_leases();
    const std::string normalized = normalize_port_forward_endpoint(endpoint);
    const SOCKET socket_value = bind_port_forward_socket(normalized, protocol);
    UniqueSocket socket(socket_value);
    const std::string bound_endpoint = socket_local_endpoint(socket.get());
    const std::string bind_id = make_port_id("bind");

    {
        BasicLockGuard lock(mutex_);
        if (protocol == "tcp") {
            tcp_listeners_[bind_id] = std::shared_ptr<SharedSocket>(
                new SharedSocket(socket.release(), lease_id)
            );
        } else if (protocol == "udp") {
            udp_sockets_[bind_id] = std::shared_ptr<SharedSocket>(
                new SharedSocket(socket.release(), lease_id)
            );
        } else {
            throw PortForwardError(
                400,
                "bad_request",
                "unsupported port forward protocol `" + protocol + "`"
            );
        }
        if (!lease_id.empty()) {
            register_bind_lease(lease_id, lease_ttl_ms, bind_id);
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
    sweep_expired_leases();
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
            new TcpConnection(accepted, listener->lease_id)
        );
        if (!listener->lease_id.empty()) {
            track_connection_lease(listener->lease_id, connection_id);
        }
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
    sweep_expired_leases();
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
        if (!listener->lease_id.empty()) {
            untrack_bind_lease(listener->lease_id, bind_id);
        }
        finish_close_shared_socket(listener);
    }
    if (socket_value.get() != NULL) {
        if (!socket_value->lease_id.empty()) {
            untrack_bind_lease(socket_value->lease_id, bind_id);
        }
        finish_close_shared_socket(socket_value);
    }
    log_message(LOG_DEBUG, "port_forward", "closed bind `" + bind_id + "`");
    return Json::object();
}

Json PortForwardStore::lease_renew(const std::string& lease_id, std::uint64_t lease_ttl_ms) {
    sweep_expired_leases();
    BasicLockGuard lock(mutex_);
    renew_lease(lease_id, lease_ttl_ms);
    return Json::object();
}

Json PortForwardStore::connect(
    const std::string& endpoint,
    const std::string& protocol,
    const std::string& lease_id,
    std::uint64_t lease_ttl_ms
) {
    sweep_expired_leases();
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
            new TcpConnection(socket_value, lease_id)
        );
        if (!lease_id.empty()) {
            register_connection_lease(lease_id, lease_ttl_ms, connection_id);
        }
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
    sweep_expired_leases();
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
    sweep_expired_leases();
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
    sweep_expired_leases();
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
        if (!connection->lease_id.empty()) {
            untrack_connection_lease(connection->lease_id, connection_id);
        }
        finish_close_tcp_connection(connection);
    }
    log_message(LOG_DEBUG, "port_forward", "closed tcp connection `" + connection_id + "`");
    return Json::object();
}

Json PortForwardStore::udp_datagram_read(const std::string& bind_id) {
    sweep_expired_leases();
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
    sweep_expired_leases();
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

void PortForwardStore::sweep_expired_leases() {
    std::vector<std::shared_ptr<SharedSocket> > expired_binds;
    std::vector<std::shared_ptr<TcpConnection> > expired_connections;
    {
        BasicLockGuard lock(mutex_);
        const std::uint64_t now = platform::monotonic_ms();
        std::vector<std::string> expired_ids;
        for (std::map<std::string, LeaseEntry>::const_iterator it = leases_.begin();
             it != leases_.end();
             ++it) {
            if (it->second.expires_at_ms <= now) {
                expired_ids.push_back(it->first);
            }
        }
        for (std::size_t i = 0; i < expired_ids.size(); ++i) {
            std::map<std::string, LeaseEntry>::iterator lease_it = leases_.find(expired_ids[i]);
            if (lease_it == leases_.end()) {
                continue;
            }
            for (std::set<std::string>::const_iterator bind_it = lease_it->second.binds.begin();
                 bind_it != lease_it->second.binds.end();
                 ++bind_it) {
                std::map<std::string, std::shared_ptr<SharedSocket> >::iterator tcp_it =
                    tcp_listeners_.find(*bind_it);
                if (tcp_it != tcp_listeners_.end()) {
                    expired_binds.push_back(tcp_it->second);
                    tcp_listeners_.erase(tcp_it);
                }
                std::map<std::string, std::shared_ptr<SharedSocket> >::iterator udp_it =
                    udp_sockets_.find(*bind_it);
                if (udp_it != udp_sockets_.end()) {
                    expired_binds.push_back(udp_it->second);
                    udp_sockets_.erase(udp_it);
                }
            }
            for (std::set<std::string>::const_iterator conn_it =
                     lease_it->second.connections.begin();
                 conn_it != lease_it->second.connections.end();
                 ++conn_it) {
                std::map<std::string, std::shared_ptr<TcpConnection> >::iterator tcp_conn_it =
                    tcp_connections_.find(*conn_it);
                if (tcp_conn_it != tcp_connections_.end()) {
                    expired_connections.push_back(tcp_conn_it->second);
                    tcp_connections_.erase(tcp_conn_it);
                }
            }
            leases_.erase(lease_it);
        }
    }

    for (std::size_t i = 0; i < expired_binds.size(); ++i) {
        finish_close_shared_socket(expired_binds[i]);
    }
    for (std::size_t i = 0; i < expired_connections.size(); ++i) {
        finish_close_tcp_connection(expired_connections[i]);
    }
}

void PortForwardStore::register_bind_lease(
    const std::string& lease_id,
    std::uint64_t lease_ttl_ms,
    const std::string& bind_id
) {
    LeaseEntry& entry = leases_[lease_id];
    entry.expires_at_ms = lease_deadline_ms(lease_ttl_ms);
    entry.binds.insert(bind_id);
}

void PortForwardStore::register_connection_lease(
    const std::string& lease_id,
    std::uint64_t lease_ttl_ms,
    const std::string& connection_id
) {
    LeaseEntry& entry = leases_[lease_id];
    entry.expires_at_ms = lease_deadline_ms(lease_ttl_ms);
    entry.connections.insert(connection_id);
}

void PortForwardStore::renew_lease(
    const std::string& lease_id,
    std::uint64_t lease_ttl_ms
) {
    std::map<std::string, LeaseEntry>::iterator it = leases_.find(lease_id);
    if (it == leases_.end()) {
        // Late renewals can race with expiry cleanup after a broker crash; treat them as a no-op.
        return;
    }
    it->second.expires_at_ms = lease_deadline_ms(lease_ttl_ms);
}

void PortForwardStore::track_connection_lease(
    const std::string& lease_id,
    const std::string& connection_id
) {
    std::map<std::string, LeaseEntry>::iterator it = leases_.find(lease_id);
    if (it != leases_.end()) {
        it->second.connections.insert(connection_id);
    }
}

void PortForwardStore::untrack_bind_lease(
    const std::string& lease_id,
    const std::string& bind_id
) {
    std::map<std::string, LeaseEntry>::iterator it = leases_.find(lease_id);
    if (it == leases_.end()) {
        return;
    }
    it->second.binds.erase(bind_id);
    if (it->second.binds.empty() && it->second.connections.empty()) {
        leases_.erase(it);
    }
}

void PortForwardStore::untrack_connection_lease(
    const std::string& lease_id,
    const std::string& connection_id
) {
    std::map<std::string, LeaseEntry>::iterator it = leases_.find(lease_id);
    if (it == leases_.end()) {
        return;
    }
    it->second.connections.erase(connection_id);
    if (it->second.binds.empty() && it->second.connections.empty()) {
        leases_.erase(it);
    }
}

std::uint64_t PortForwardStore::lease_deadline_ms(std::uint64_t lease_ttl_ms) const {
    if (lease_ttl_ms == 0U) {
        throw PortForwardError(400, "invalid_port_lease", "port forward lease ttl_ms must be > 0");
    }
    const std::uint64_t ttl_ms = lease_ttl_ms < MIN_LEASE_TTL_MS ? MIN_LEASE_TTL_MS : lease_ttl_ms;
    return platform::monotonic_ms() + ttl_ms;
}
