#include "port_forward.h"

#include <algorithm>
#include <atomic>
#include <cctype>
#include <cstdio>
#include <cstring>
#include <limits>
#include <sstream>
#include <vector>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#else
#include <arpa/inet.h>
#include <netdb.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>
#endif

#include "logging.h"
#include "text_utils.h"

namespace {

const std::size_t READ_BUF_SIZE = 64U * 1024U;

std::atomic<unsigned long> next_port_id(1UL);

std::string make_port_id(const char* prefix) {
    std::ostringstream out;
    out << prefix << '_' << next_port_id.fetch_add(1UL);
    return out.str();
}

bool all_ascii_digits(const std::string& value) {
    if (value.empty()) {
        return false;
    }
    for (std::size_t i = 0; i < value.size(); ++i) {
        if (!std::isdigit(static_cast<unsigned char>(value[i]))) {
            return false;
        }
    }
    return true;
}

unsigned long parse_port_number(const std::string& value) {
    if (value.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "invalid port `" + value + "`");
    }
    unsigned long port = 0;
    for (std::size_t i = 0; i < value.size(); ++i) {
        const char ch = value[i];
        if (ch < '0' || ch > '9') {
            throw PortForwardError(400, "invalid_endpoint", "invalid port `" + value + "`");
        }
        const unsigned long digit = static_cast<unsigned long>(ch - '0');
        if (port > (65535UL - digit) / 10UL) {
            throw PortForwardError(400, "invalid_endpoint", "invalid port `" + value + "`");
        }
        port = port * 10UL + digit;
    }
    return port;
}

void split_host_port(
    const std::string& endpoint,
    std::string* host,
    std::string* port
) {
    if (!endpoint.empty() && endpoint[0] == '[') {
        const std::size_t close = endpoint.find(']');
        if (close == std::string::npos) {
            throw PortForwardError(
                400,
                "invalid_endpoint",
                "invalid endpoint `" + endpoint + "`; missing `]`"
            );
        }
        if (close + 1U >= endpoint.size() || endpoint[close + 1U] != ':') {
            throw PortForwardError(
                400,
                "invalid_endpoint",
                "invalid endpoint `" + endpoint + "`; expected [host]:port"
            );
        }
        *host = endpoint.substr(1, close - 1U);
        *port = endpoint.substr(close + 2U);
        return;
    }

    const std::size_t colon = endpoint.rfind(':');
    if (colon == std::string::npos) {
        throw PortForwardError(
            400,
            "invalid_endpoint",
            "invalid endpoint `" + endpoint + "`; expected <port> or <host>:<port>"
        );
    }
    *host = endpoint.substr(0, colon);
    *port = endpoint.substr(colon + 1U);
}

unsigned long endpoint_port(const std::string& endpoint) {
    std::string host;
    std::string port;
    split_host_port(endpoint, &host, &port);
    return parse_port_number(port);
}

std::string printable_endpoint(
    const sockaddr* address,
    socklen_t address_len
) {
    char host[NI_MAXHOST];
    char service[NI_MAXSERV];
    const int result = getnameinfo(
        address,
        address_len,
        host,
        sizeof(host),
        service,
        sizeof(service),
        NI_NUMERICHOST | NI_NUMERICSERV
    );
    if (result != 0) {
        return "unknown:0";
    }

    if (address->sa_family == AF_INET6) {
        return "[" + std::string(host) + "]:" + std::string(service);
    }
    return std::string(host) + ":" + std::string(service);
}

std::string socket_local_endpoint(SOCKET socket) {
    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    socklen_t address_len = sizeof(address);
    if (getsockname(socket, reinterpret_cast<sockaddr*>(&address), &address_len) != 0) {
        throw PortForwardError(400, "port_bind_failed", socket_error_message("getsockname"));
    }
    return printable_endpoint(reinterpret_cast<sockaddr*>(&address), address_len);
}

int protocol_to_socktype(const std::string& protocol) {
    if (protocol == "tcp") {
        return SOCK_STREAM;
    }
    if (protocol == "udp") {
        return SOCK_DGRAM;
    }
    throw PortForwardError(
        400,
        "bad_request",
        "unsupported port forward protocol `" + protocol + "`"
    );
}

int protocol_to_ipproto(const std::string& protocol) {
    if (protocol == "tcp") {
        return IPPROTO_TCP;
    }
    if (protocol == "udp") {
        return IPPROTO_UDP;
    }
    throw PortForwardError(
        400,
        "bad_request",
        "unsupported port forward protocol `" + protocol + "`"
    );
}

void endpoint_to_host_port(
    const std::string& endpoint,
    std::string* host,
    std::string* port
) {
    split_host_port(endpoint, host, port);
    if (host->empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint host must not be empty");
    }
    parse_port_number(*port);
}

addrinfo* resolve_endpoint(
    const std::string& endpoint,
    const std::string& protocol,
    int flags,
    const char* error_code
) {
    std::string host;
    std::string port;
    endpoint_to_host_port(endpoint, &host, &port);

    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = protocol_to_socktype(protocol);
    hints.ai_protocol = protocol_to_ipproto(protocol);
    hints.ai_flags = flags;

    addrinfo* result = NULL;
    const int status = getaddrinfo(host.c_str(), port.c_str(), &hints, &result);
    if (status != 0 || result == NULL) {
        std::ostringstream message;
        message << "resolving endpoint `" << endpoint << "` failed";
#ifdef _WIN32
        message << ": " << status;
#else
        message << ": " << gai_strerror(status);
#endif
        throw PortForwardError(400, error_code, message.str());
    }
    return result;
}

SOCKET bind_socket(const std::string& endpoint, const std::string& protocol) {
    addrinfo* result = resolve_endpoint(endpoint, protocol, AI_PASSIVE, "invalid_endpoint");
    SOCKET bound_socket = INVALID_SOCKET;

    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        bound_socket = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (bound_socket == INVALID_SOCKET) {
            continue;
        }

        int yes = 1;
        setsockopt(
            bound_socket,
            SOL_SOCKET,
            SO_REUSEADDR,
            reinterpret_cast<const char*>(&yes),
            sizeof(yes)
        );

        if (bind(bound_socket, current->ai_addr, static_cast<int>(current->ai_addrlen)) == 0) {
            break;
        }

        close_socket(bound_socket);
        bound_socket = INVALID_SOCKET;
    }

    freeaddrinfo(result);

    if (bound_socket == INVALID_SOCKET) {
        throw PortForwardError(400, "port_bind_failed", socket_error_message("bind"));
    }

    if (protocol == "tcp" && listen(bound_socket, SOMAXCONN) != 0) {
        const std::string message = socket_error_message("listen");
        close_socket(bound_socket);
        throw PortForwardError(400, "port_bind_failed", message);
    }

    return bound_socket;
}

SOCKET connect_socket(const std::string& endpoint, const std::string& protocol) {
    addrinfo* result = resolve_endpoint(endpoint, protocol, 0, "invalid_endpoint");
    SOCKET connected_socket = INVALID_SOCKET;

    for (addrinfo* current = result; current != NULL; current = current->ai_next) {
        connected_socket = socket(current->ai_family, current->ai_socktype, current->ai_protocol);
        if (connected_socket == INVALID_SOCKET) {
            continue;
        }

        if (connect(
                connected_socket,
                current->ai_addr,
                static_cast<int>(current->ai_addrlen)
            ) == 0) {
            break;
        }

        close_socket(connected_socket);
        connected_socket = INVALID_SOCKET;
    }

    freeaddrinfo(result);

    if (connected_socket == INVALID_SOCKET) {
        throw PortForwardError(400, "port_connect_failed", socket_error_message("connect"));
    }

    return connected_socket;
}

int base64_value(unsigned char ch) {
    if (ch >= 'A' && ch <= 'Z') {
        return static_cast<int>(ch - 'A');
    }
    if (ch >= 'a' && ch <= 'z') {
        return static_cast<int>(ch - 'a') + 26;
    }
    if (ch >= '0' && ch <= '9') {
        return static_cast<int>(ch - '0') + 52;
    }
    if (ch == '+') {
        return 62;
    }
    if (ch == '/') {
        return 63;
    }
    return -1;
}

std::vector<unsigned char> decode_base64_values(const std::string& data) {
    if (data.size() % 4U != 0U) {
        throw PortForwardError(400, "invalid_port_data", "invalid base64 length");
    }

    std::vector<unsigned char> bytes;
    bytes.reserve((data.size() / 4U) * 3U);

    for (std::size_t offset = 0; offset < data.size(); offset += 4U) {
        int values[4];
        int padding = 0;
        for (std::size_t index = 0; index < 4U; ++index) {
            const unsigned char ch = static_cast<unsigned char>(data[offset + index]);
            if (ch == '=') {
                values[index] = 0;
                ++padding;
            } else {
                const int value = base64_value(ch);
                if (value < 0) {
                    throw PortForwardError(400, "invalid_port_data", "invalid base64 data");
                }
                values[index] = value;
            }
        }
        if (padding > 2 || (padding > 0 && offset + 4U != data.size())) {
            throw PortForwardError(400, "invalid_port_data", "invalid base64 padding");
        }
        bytes.push_back(static_cast<unsigned char>((values[0] << 2) | (values[1] >> 4)));
        if (padding < 2) {
            bytes.push_back(
                static_cast<unsigned char>(((values[1] & 0x0f) << 4) | (values[2] >> 2))
            );
        }
        if (padding < 1) {
            bytes.push_back(
                static_cast<unsigned char>(((values[2] & 0x03) << 6) | values[3])
            );
        }
    }

    return bytes;
}

std::string bytes_to_string(const std::vector<unsigned char>& bytes) {
    if (bytes.empty()) {
        return "";
    }
    return std::string(
        reinterpret_cast<const char*>(bytes.data()),
        reinterpret_cast<const char*>(bytes.data()) + bytes.size()
    );
}

void send_all_socket(SOCKET socket, const std::string& data) {
    std::size_t offset = 0;
    while (offset < data.size()) {
        const int sent = send(
            socket,
            data.data() + offset,
            static_cast<int>(data.size() - offset),
            0
        );
        if (sent <= 0) {
            throw PortForwardError(400, "port_write_failed", socket_error_message("send"));
        }
        offset += static_cast<std::size_t>(sent);
    }
}

sockaddr_storage parse_peer_endpoint(const std::string& peer, socklen_t* peer_len) {
    addrinfo* result = resolve_endpoint(peer, "udp", 0, "invalid_endpoint");
    sockaddr_storage address;
    std::memset(&address, 0, sizeof(address));
    *peer_len = 0;
    if (result != NULL) {
        std::memcpy(&address, result->ai_addr, result->ai_addrlen);
        *peer_len = static_cast<socklen_t>(result->ai_addrlen);
    }
    freeaddrinfo(result);
    return address;
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

TcpConnection::TcpConnection(SOCKET socket_value) : socket(socket_value) {}

SharedSocket::SharedSocket(SOCKET socket_value) : socket(socket_value) {}

PortForwardStore::PortForwardStore() {}

PortForwardStore::~PortForwardStore() {}

Json PortForwardStore::listen(const std::string& endpoint, const std::string& protocol) {
    const std::string normalized = normalize_port_forward_endpoint(endpoint);
    const SOCKET socket_value = bind_socket(normalized, protocol);
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
        throw PortForwardError(400, "port_accept_failed", socket_error_message("accept"));
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
            printable_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len) + "`"
    );
    return Json{{"connection_id", connection_id}};
}

Json PortForwardStore::listen_close(const std::string& bind_id) {
    BasicLockGuard lock(mutex_);
    tcp_listeners_.erase(bind_id);
    udp_sockets_.erase(bind_id);
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
    const SOCKET socket_value = connect_socket(normalized, protocol);
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

    std::string buffer;
    buffer.resize(READ_BUF_SIZE);
    const int received = recv(
        connection->socket.get(),
        &buffer[0],
        static_cast<int>(buffer.size()),
        0
    );
    if (received < 0) {
        throw PortForwardError(400, "port_read_failed", socket_error_message("recv"));
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
    send_all_socket(connection->socket.get(), bytes);
    return Json::object();
}

Json PortForwardStore::connection_close(const std::string& connection_id) {
    BasicLockGuard lock(mutex_);
    tcp_connections_.erase(connection_id);
    log_message(LOG_DEBUG, "port_forward", "closed tcp connection `" + connection_id + "`");
    return Json::object();
}

Json PortForwardStore::udp_datagram_read(const std::string& bind_id) {
    const std::shared_ptr<SharedSocket> socket_value = udp_socket(bind_id);
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
        throw PortForwardError(400, "port_read_failed", socket_error_message("recvfrom"));
    }
    buffer.resize(static_cast<std::size_t>(received));
    return Json{
        {"peer", printable_endpoint(reinterpret_cast<sockaddr*>(&peer_address), peer_len)},
        {"data", base64_encode_bytes(bytes_to_string(buffer))},
    };
}

Json PortForwardStore::udp_datagram_write(
    const std::string& bind_id,
    const std::string& peer,
    const std::string& data
) {
    const std::shared_ptr<SharedSocket> socket_value = udp_socket(bind_id);
    const std::string bytes = base64_decode_bytes(data);
    socklen_t peer_len = 0;
    const sockaddr_storage peer_address = parse_peer_endpoint(peer, &peer_len);
    const int sent = sendto(
        socket_value->socket.get(),
        bytes.data(),
        static_cast<int>(bytes.size()),
        0,
        reinterpret_cast<const sockaddr*>(&peer_address),
        peer_len
    );
    if (sent < 0 || static_cast<std::size_t>(sent) != bytes.size()) {
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

std::string normalize_port_forward_endpoint(const std::string& endpoint) {
    const std::string trimmed = trim_ascii(endpoint);
    if (trimmed.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint must not be empty");
    }
    if (all_ascii_digits(trimmed)) {
        const unsigned long port = parse_port_number(trimmed);
        std::ostringstream normalized;
        normalized << "127.0.0.1:" << port;
        return normalized.str();
    }

    std::string host;
    std::string port;
    split_host_port(trimmed, &host, &port);
    if (host.empty()) {
        throw PortForwardError(400, "invalid_endpoint", "endpoint host must not be empty");
    }
    parse_port_number(port);
    return trimmed;
}

std::string ensure_nonzero_connect_endpoint(const std::string& endpoint) {
    const std::string normalized = normalize_port_forward_endpoint(endpoint);
    if (endpoint_port(normalized) == 0UL) {
        throw PortForwardError(
            400,
            "invalid_endpoint",
            "connect_endpoint `" + normalized + "` must use a nonzero port"
        );
    }
    return normalized;
}

std::string base64_encode_bytes(const std::string& bytes) {
    static const char alphabet[] =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string output;
    output.reserve(((bytes.size() + 2U) / 3U) * 4U);
    for (std::size_t offset = 0; offset < bytes.size(); offset += 3U) {
        const unsigned int octet_a = static_cast<unsigned char>(bytes[offset]);
        const unsigned int octet_b =
            offset + 1U < bytes.size() ? static_cast<unsigned char>(bytes[offset + 1U]) : 0U;
        const unsigned int octet_c =
            offset + 2U < bytes.size() ? static_cast<unsigned char>(bytes[offset + 2U]) : 0U;
        const unsigned int triple = (octet_a << 16) | (octet_b << 8) | octet_c;

        output.push_back(alphabet[(triple >> 18) & 0x3f]);
        output.push_back(alphabet[(triple >> 12) & 0x3f]);
        output.push_back(offset + 1U < bytes.size() ? alphabet[(triple >> 6) & 0x3f] : '=');
        output.push_back(offset + 2U < bytes.size() ? alphabet[triple & 0x3f] : '=');
    }
    return output;
}

std::string base64_decode_bytes(const std::string& data) {
    const std::vector<unsigned char> bytes = decode_base64_values(data);
    return bytes_to_string(bytes);
}
