#include "test_assert.h"

#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <map>
#include <string>
#include <thread>
#include <vector>

#ifndef _WIN32
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>
#endif

#include "core/byte_buffer.h"
#include "core/event_loop.h"
#include "core/json_value.h"
#include "core/stream.h"
#include "platform/platform.h"
#include "platform/socket.h"
#include "port_forward/port_tunnel_frame.h"
#include "rpc/server_contract.h"
#include "runtime/reactor_server.h"
#include "test_tar_helpers.h"
#include "tls/tls_connection_transport.h"
namespace {

void test_byte_buffer() {
    ByteBuffer buffer(4U);
    TEST_ASSERT(buffer.append("abc", 3U));
    TEST_ASSERT(!buffer.append("de", 2U));
    TEST_ASSERT(buffer.size() == 3U);
    buffer.consume(1U);
    TEST_ASSERT(std::string(buffer.data(), buffer.size()) == "bc");
    TEST_ASSERT(buffer.append("de", 2U));
    TEST_ASSERT(buffer.size() == 4U);
    TEST_ASSERT(buffer.take() == "bcde");
    TEST_ASSERT(buffer.empty());
}

void test_json_depth_limit() {
    bool parsed = false;
    try {
        (void)parse_request_json_object("{\"cmd\":\"dir\"}");
        parsed = true;
    } catch (const JsonRequestError&) {
    }
    TEST_ASSERT(parsed);

    bool rejected = false;
    try {
        (void)parse_request_json_object("{\"cmd\":");
    } catch (const JsonRequestError&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);

    std::string deep = "{\"x\":";
    for (int i = 0; i < 40; ++i) {
        deep += "[";
    }
    for (int i = 0; i < 40; ++i) {
        deep += "]";
    }
    deep += "}";
    rejected = false;
    try {
        (void)parse_request_json_object(deep);
    } catch (const JsonRequestError&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);

    rejected = false;
    try {
        (void)parse_request_json_object("[]");
    } catch (const JsonRequestError&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);
}

#ifndef _WIN32
void test_event_loop() {
    EventLoop loop;
    TEST_ASSERT(loop.valid());

    int fds[2];
    TEST_ASSERT(socketpair(AF_UNIX, SOCK_STREAM, 0, fds) == 0);

    std::atomic<bool> timer_fired(false);
    std::atomic<bool> post_fired(false);
    std::atomic<bool> read_fired(false);

    TEST_ASSERT(loop.add(fds[0], IO_READ, [&](int) {
        char byte = 0;
        if (read(fds[0], &byte, 1U) == 1) {
            read_fired.store(true);
        }
    }));

    std::thread loop_thread([&]() { loop.run(); });

    loop.add_timer(platform::monotonic_ms() + 20U, [&]() { timer_fired.store(true); });
    loop.post([&]() { post_fired.store(true); });

    const char byte = 'x';
    TEST_ASSERT(write(fds[1], &byte, 1U) == 1);

    TEST_ASSERT(test_assert::wait_until_true(timer_fired, 2000U));
    TEST_ASSERT(test_assert::wait_until_true(post_fired, 2000U));
    TEST_ASSERT(test_assert::wait_until_true(read_fired, 2000U));

    loop.stop();
    loop_thread.join();
    close(fds[0]);
    close(fds[1]);
}

struct Response {
    Response() : status(0) {}
    int status;
    std::string body;
};

int connect_local(unsigned short port) {
    const int fd = create_socket_cloexec(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    TEST_ASSERT(fd != INVALID_SOCKET);
    sockaddr_in address;
    std::memset(&address, 0, sizeof(address));
    address.sin_family = AF_INET;
    address.sin_port = htons(port);
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (connect_socket(fd, reinterpret_cast<sockaddr*>(&address), sizeof(address)) != 0) {
        close_socket(fd);
        TEST_FAIL_MESSAGE("connect to reactor server failed");
    }
    socket_set_nonblocking(fd, false);
    return fd;
}

void send_all_text(int fd, const std::string& text) {
    std::size_t offset = 0U;
    while (offset < text.size()) {
        const int sent = send_bounded(fd, text.data() + offset, text.size() - offset, 0);
        TEST_ASSERT(sent > 0);
        offset += static_cast<std::size_t>(sent);
    }
}

std::size_t find_header_end(const std::string& data) {
    return data.find("\r\n\r\n");
}

Response read_response(int fd) {
    std::string data;
    char buffer[4096];
    std::size_t header_end = std::string::npos;
    while (header_end == std::string::npos) {
        const int received = recv_bounded(fd, buffer, sizeof(buffer), 0);
        TEST_ASSERT(received > 0);
        data.append(buffer, static_cast<std::size_t>(received));
        header_end = find_header_end(data);
    }

    Response response;
    TEST_ASSERT(std::sscanf(data.c_str(), "HTTP/1.1 %d", &response.status) == 1);

    std::size_t content_length = 0U;
    const std::size_t length_position = data.find("Content-Length: ");
    TEST_ASSERT(length_position != std::string::npos);
    content_length =
        static_cast<std::size_t>(std::strtoul(data.c_str() + length_position + 16U, nullptr, 10));

    const std::size_t body_offset = header_end + 4U;
    while (data.size() < body_offset + content_length) {
        const int received = recv_bounded(fd, buffer, sizeof(buffer), 0);
        TEST_ASSERT(received > 0);
        data.append(buffer, static_cast<std::size_t>(received));
    }
    response.body = data.substr(body_offset, content_length);
    return response;
}

Response request_with_headers(
    unsigned short port,
    const char* path,
    const std::string& body,
    const std::map<std::string, std::string>& headers,
    bool chunked
) {
    const int fd = connect_local(port);
    std::string request_text = std::string("POST ") + path + " HTTP/1.1\r\n";
    request_text += "Host: 127.0.0.1\r\n";
    request_text += "Connection: close\r\n";
    for (std::map<std::string, std::string>::const_iterator it = headers.begin();
         it != headers.end();
         ++it) {
        request_text += it->first + ": " + it->second + "\r\n";
    }
    std::string payload = body;
    if (chunked) {
        request_text += "Transfer-Encoding: chunked\r\n";
        payload.clear();
        const std::size_t chunk_size = 4096U;
        for (std::size_t offset = 0; offset < body.size(); offset += chunk_size) {
            const std::size_t size = std::min(chunk_size, body.size() - offset);
            char header[32];
            std::snprintf(header, sizeof(header), "%zx", size);
            payload += header;
            payload += "\r\n";
            payload.append(body.data() + offset, size);
            payload += "\r\n";
        }
        payload += "0\r\n\r\n";
    } else {
        request_text += "Content-Length: " + std::to_string(body.size()) + "\r\n";
    }
    request_text += "\r\n";
    request_text += payload;
    send_all_text(fd, request_text);
    const Response response = read_response(fd);
    close_socket(fd);
    return response;
}

Response request(unsigned short port, const char* path, const std::string& body) {
    return request_with_headers(port, path, body, std::map<std::string, std::string>(), false);
}

Json response_json(const Response& response) {
    return parse_request_json_object(response.body);
}

void send_bytes(int fd, const char* data, std::size_t size) {
    std::size_t offset = 0U;
    while (offset < size) {
        const int sent = send_bounded(fd, data + offset, size - offset, 0);
        TEST_ASSERT(sent > 0);
        offset += static_cast<std::size_t>(sent);
    }
}

bool read_exact(int fd, char* data, std::size_t size) {
    std::size_t offset = 0U;
    while (offset < size) {
        const int received = recv_bounded(fd, data + offset, size - offset, 0);
        if (received <= 0) {
            return false;
        }
        offset += static_cast<std::size_t>(received);
    }
    return true;
}

PortTunnelFrame tunnel_frame(
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

struct TunnelClient {
    explicit TunnelClient(unsigned short port) : fd(connect_local(port)) {
        set_socket_timeout_ms(fd, 5000);
        const std::string request = "POST /v1/port/tunnel HTTP/1.1\r\n"
                                    "Host: 127.0.0.1\r\n"
                                    "Connection: Upgrade\r\n"
                                    "Upgrade: remote-exec-port-tunnel\r\n"
                                    "x-remote-exec-port-tunnel-version: 4\r\n"
                                    "Content-Length: 0\r\n\r\n";
        send_bytes(fd, request.data(), request.size());

        std::string head;
        char byte = 0;
        while (head.find("\r\n\r\n") == std::string::npos) {
            TEST_ASSERT(read_exact(fd, &byte, 1U));
            head.push_back(byte);
        }
        TEST_ASSERT(head.find("HTTP/1.1 101") == 0U);
    }

    ~TunnelClient() { close_socket(fd); }

    void send(const PortTunnelFrame& frame) {
        const std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        send_bytes(fd, reinterpret_cast<const char*>(bytes.data()), bytes.size());
    }

    PortTunnelFrame read() {
        unsigned char header[PORT_TUNNEL_HEADER_LEN];
        TEST_ASSERT(read_exact(fd, reinterpret_cast<char*>(header), sizeof(header)));
        const std::uint32_t meta_len = (static_cast<std::uint32_t>(header[8]) << 24)
                                       | (static_cast<std::uint32_t>(header[9]) << 16)
                                       | (static_cast<std::uint32_t>(header[10]) << 8)
                                       | static_cast<std::uint32_t>(header[11]);
        const std::uint32_t data_len = (static_cast<std::uint32_t>(header[12]) << 24)
                                       | (static_cast<std::uint32_t>(header[13]) << 16)
                                       | (static_cast<std::uint32_t>(header[14]) << 8)
                                       | static_cast<std::uint32_t>(header[15]);
        std::vector<unsigned char> bytes(header, header + PORT_TUNNEL_HEADER_LEN);
        bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
        TEST_ASSERT(read_exact(
            fd,
            reinterpret_cast<char*>(bytes.data() + PORT_TUNNEL_HEADER_LEN),
            meta_len + data_len
        ));
        return decode_port_tunnel_frame(bytes);
    }

    int fd;
};

unsigned short endpoint_port(const std::string& endpoint) {
    const std::size_t colon = endpoint.rfind(':');
    TEST_ASSERT(colon != std::string::npos);
    return static_cast<unsigned short>(std::strtoul(endpoint.c_str() + colon + 1U, nullptr, 10));
}

struct EchoServer {
    EchoServer() : listener(INVALID_SOCKET), port(0U) {
        listener = create_socket_cloexec(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        TEST_ASSERT(listener != INVALID_SOCKET);
        sockaddr_in address;
        std::memset(&address, 0, sizeof(address));
        address.sin_family = AF_INET;
        address.sin_port = 0;
        address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        TEST_ASSERT(
            bind_socket(listener, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0
        );
        TEST_ASSERT(listen_socket(listener, 1) == 0);
        socklen_t address_len = sizeof(address);
        TEST_ASSERT(
            getsockname(listener, reinterpret_cast<sockaddr*>(&address), &address_len) == 0
        );
        port = ntohs(address.sin_port);
        thread = std::thread([this]() {
            const int accepted = accept_socket_cloexec(listener, nullptr, nullptr);
            char buffer[64];
            const int received =
                accepted == INVALID_SOCKET ? -1 : recv_bounded(accepted, buffer, sizeof(buffer), 0);
            if (received > 0) {
                (void)send_bounded(accepted, buffer, static_cast<std::size_t>(received), 0);
            }
            if (accepted != INVALID_SOCKET) {
                close_socket(accepted);
            }
        });
    }

    ~EchoServer() {
        if (thread.joinable()) {
            thread.join();
        }
        if (listener != INVALID_SOCKET) {
            close_socket(listener);
        }
    }

    int listener;
    unsigned short port;
    std::thread thread;
};

PortTunnelFrame open_frame(const char* role, const char* protocol) {
    return tunnel_frame(
        PortTunnelFrameType::TunnelOpen,
        0U,
        Json{{"role", role}, {"protocol", protocol}, {"generation", 1}},
        std::string()
    );
}

void test_tunnel_connect(unsigned short port) {
    EchoServer echo;
    TunnelClient tunnel(port);
    const std::string preface(port_tunnel_preface(), port_tunnel_preface_size());
    send_bytes(tunnel.fd, preface.data(), preface.size());

    tunnel.send(open_frame("connect", "tcp"));
    const PortTunnelFrame ready = tunnel.read();
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);

    tunnel.send(tunnel_frame(
        PortTunnelFrameType::TcpConnect,
        2U,
        Json{{"endpoint", "127.0.0.1:" + std::to_string(echo.port)}},
        std::string()
    ));
    const PortTunnelFrame connected = tunnel.read();
    TEST_ASSERT(connected.type == PortTunnelFrameType::TcpConnectOk);
    TEST_ASSERT(connected.stream_id == 2U);

    tunnel.send(tunnel_frame(PortTunnelFrameType::TcpData, 2U, Json::object(), "ping"));
    const PortTunnelFrame echoed = tunnel.read();
    TEST_ASSERT(echoed.type == PortTunnelFrameType::TcpData);
    TEST_ASSERT(echoed.stream_id == 2U);
    TEST_ASSERT(std::string(echoed.data.begin(), echoed.data.end()) == "ping");

    tunnel.send(tunnel_frame(PortTunnelFrameType::Close, 2U, Json::object(), std::string()));
}

void test_tunnel_listen(unsigned short port) {
    TunnelClient tunnel(port);
    const std::string preface(port_tunnel_preface(), port_tunnel_preface_size());
    send_bytes(tunnel.fd, preface.data(), preface.size());

    tunnel.send(open_frame("listen", "tcp"));
    const PortTunnelFrame ready = tunnel.read();
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);
    const Json ready_meta = Json::parse(ready.meta);
    TEST_ASSERT(ready_meta.contains("session_id"));
    TEST_ASSERT(ready_meta.at("resume_timeout_ms").get<std::uint64_t>() > 0U);

    tunnel.send(tunnel_frame(
        PortTunnelFrameType::TcpListen,
        1U,
        Json{{"endpoint", "127.0.0.1:0"}},
        std::string()
    ));
    const PortTunnelFrame listen_ok = tunnel.read();
    TEST_ASSERT(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const unsigned short listen_port =
        endpoint_port(Json::parse(listen_ok.meta).at("endpoint").get<std::string>());

    const int client = connect_local(listen_port);
    const PortTunnelFrame accepted = tunnel.read();
    TEST_ASSERT(accepted.type == PortTunnelFrameType::TcpAccept);
    const std::uint32_t accepted_stream = accepted.stream_id;

    send_bytes(client, "world", 5U);
    const PortTunnelFrame inbound = tunnel.read();
    TEST_ASSERT(inbound.type == PortTunnelFrameType::TcpData);
    TEST_ASSERT(inbound.stream_id == accepted_stream);
    TEST_ASSERT(std::string(inbound.data.begin(), inbound.data.end()) == "world");

    tunnel.send(tunnel_frame(PortTunnelFrameType::TcpData, accepted_stream, Json::object(), "hello")
    );
    char buffer[5];
    TEST_ASSERT(read_exact(client, buffer, sizeof(buffer)));
    TEST_ASSERT(std::string(buffer, sizeof(buffer)) == "hello");

    tunnel.send(
        tunnel_frame(PortTunnelFrameType::Close, accepted_stream, Json::object(), std::string())
    );
    close_socket(client);
}

void test_tunnel_bind_conflict(unsigned short port) {
    const int occupied = create_socket_cloexec(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    TEST_ASSERT(occupied != INVALID_SOCKET);
    sockaddr_in address;
    std::memset(&address, 0, sizeof(address));
    address.sin_family = AF_INET;
    address.sin_port = 0;
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    TEST_ASSERT(bind_socket(occupied, reinterpret_cast<sockaddr*>(&address), sizeof(address)) == 0);
    TEST_ASSERT(listen_socket(occupied, 1) == 0);
    socklen_t address_len = sizeof(address);
    TEST_ASSERT(getsockname(occupied, reinterpret_cast<sockaddr*>(&address), &address_len) == 0);
    const unsigned short occupied_port = ntohs(address.sin_port);

    TunnelClient tunnel(port);
    const std::string preface(port_tunnel_preface(), port_tunnel_preface_size());
    send_bytes(tunnel.fd, preface.data(), preface.size());
    tunnel.send(open_frame("listen", "tcp"));
    const PortTunnelFrame ready = tunnel.read();
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);

    tunnel.send(tunnel_frame(
        PortTunnelFrameType::TcpListen,
        1U,
        Json{{"endpoint", "127.0.0.1:" + std::to_string(occupied_port)}},
        std::string()
    ));
    const PortTunnelFrame failed = tunnel.read();
    TEST_ASSERT(failed.type == PortTunnelFrameType::Error);
    TEST_ASSERT(failed.stream_id == 1U);
    TEST_ASSERT(Json::parse(failed.meta).at("code").get<std::string>() == "listener_open_failed");
    close_socket(occupied);
}

void test_tunnel_udp(unsigned short port) {
    TunnelClient tunnel(port);
    const std::string preface(port_tunnel_preface(), port_tunnel_preface_size());
    send_bytes(tunnel.fd, preface.data(), preface.size());

    tunnel.send(open_frame("listen", "udp"));
    const PortTunnelFrame ready = tunnel.read();
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);

    tunnel.send(tunnel_frame(
        PortTunnelFrameType::UdpBind,
        1U,
        Json{{"endpoint", "127.0.0.1:0"}},
        std::string()
    ));
    const PortTunnelFrame bind_ok = tunnel.read();
    TEST_ASSERT(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const unsigned short bind_port =
        endpoint_port(Json::parse(bind_ok.meta).at("endpoint").get<std::string>());

    const int udp = create_socket_cloexec(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    TEST_ASSERT(udp != INVALID_SOCKET);
    set_socket_timeout_ms(udp, 5000);
    sockaddr_in destination;
    std::memset(&destination, 0, sizeof(destination));
    destination.sin_family = AF_INET;
    destination.sin_port = htons(bind_port);
    destination.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    TEST_ASSERT(
        sendto(udp, "u1", 2U, 0, reinterpret_cast<sockaddr*>(&destination), sizeof(destination))
        == 2
    );

    const PortTunnelFrame inbound = tunnel.read();
    TEST_ASSERT(inbound.type == PortTunnelFrameType::UdpDatagram);
    TEST_ASSERT(std::string(inbound.data.begin(), inbound.data.end()) == "u1");
    const std::string peer = Json::parse(inbound.meta).at("peer").get<std::string>();

    tunnel.send(tunnel_frame(PortTunnelFrameType::UdpDatagram, 1U, Json{{"peer", peer}}, "u2"));
    char buffer[8];
    sockaddr_storage source;
    socklen_t source_len = sizeof(source);
    const int received =
        recvfrom(udp, buffer, sizeof(buffer), 0, reinterpret_cast<sockaddr*>(&source), &source_len);
    TEST_ASSERT(received == 2);
    TEST_ASSERT(std::string(buffer, 2U) == "u2");
    close_socket(udp);
}

void test_tunnel(unsigned short port) {
    test_tunnel_connect(port);
    test_tunnel_listen(port);
    test_tunnel_bind_conflict(port);
    test_tunnel_udp(port);
}

void test_reverse() {
    const int broker_listener = create_socket_cloexec(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    TEST_ASSERT(broker_listener != INVALID_SOCKET);
    sockaddr_in broker_address;
    std::memset(&broker_address, 0, sizeof(broker_address));
    broker_address.sin_family = AF_INET;
    broker_address.sin_port = 0;
    broker_address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    TEST_ASSERT(
        bind_socket(
            broker_listener,
            reinterpret_cast<sockaddr*>(&broker_address),
            sizeof(broker_address)
        )
        == 0
    );
    TEST_ASSERT(listen_socket(broker_listener, 1) == 0);
    socklen_t broker_address_len = sizeof(broker_address);
    TEST_ASSERT(
        getsockname(
            broker_listener,
            reinterpret_cast<sockaddr*>(&broker_address),
            &broker_address_len
        )
        == 0
    );
    const unsigned short broker_port = ntohs(broker_address.sin_port);

    DaemonConfig config;
    config.connection_mode = ConnectionMode::Reverse;
    config.target = "reactor-reverse-test";
    config.default_workdir = "/tmp";
    config.max_connections = 8U;
    config.blocking_workers = 2U;
    config.reverse_broker_host = "127.0.0.1";
    config.reverse_broker_port = static_cast<int>(broker_port);
    config.reverse_bearer_token = "reverse-secret";
    config.reverse_min_idle_connections = 1U;
    config.reverse_max_connections = 1U;
    config.reverse_reconnect_ms = 100U;

    ReactorServer server(config);
    std::string start_error;
    TEST_ASSERT(server.start(&start_error));
    std::thread server_thread([&]() { server.run(); });

    std::atomic<bool> broker_ok(false);
    std::thread broker_thread([&]() {
        const int accepted = accept_socket_cloexec(broker_listener, nullptr, nullptr);
        if (accepted == INVALID_SOCKET) {
            return;
        }
        set_socket_timeout_ms(accepted, 5000);
        char magic[8];
        char length[4];
        if (!read_exact(accepted, magic, sizeof(magic))
            || std::string(magic, sizeof(magic)) != "REXREV1\n"
            || !read_exact(accepted, length, sizeof(length))) {
            close_socket(accepted);
            return;
        }
        const std::uint32_t size =
            (static_cast<std::uint32_t>(static_cast<unsigned char>(length[0])) << 24)
            | (static_cast<std::uint32_t>(static_cast<unsigned char>(length[1])) << 16)
            | (static_cast<std::uint32_t>(static_cast<unsigned char>(length[2])) << 8)
            | static_cast<std::uint32_t>(static_cast<unsigned char>(length[3]));
        std::string payload(size, '\0');
        if (!read_exact(accepted, &payload[0], payload.size())) {
            close_socket(accepted);
            return;
        }
        if (payload.find("\"target\":\"reactor-reverse-test\"") == std::string::npos
            || payload.find("\"bearer_token\":\"reverse-secret\"") == std::string::npos) {
            close_socket(accepted);
            return;
        }
        const std::string ack_payload = "{\"accepted\":true}";
        std::string ack = "REXREV1\n";
        ack.push_back('\0');
        ack.push_back('\0');
        ack.push_back('\0');
        ack.push_back(static_cast<char>(ack_payload.size()));
        ack += ack_payload;
        send_bytes(accepted, ack.data(), ack.size());

        const std::string request = "POST /v1/health HTTP/1.1\r\n"
                                    "Host: 127.0.0.1\r\n"
                                    "Connection: close\r\n"
                                    "Content-Length: 2\r\n\r\n{}";
        send_bytes(accepted, request.data(), request.size());
        const Response response = read_response(accepted);
        broker_ok.store(response.status == 200);
        close_socket(accepted);
    });

    TEST_ASSERT(test_assert::wait_until_true(broker_ok, 8000U));
    server.request_stop();
    server_thread.join();
    broker_thread.join();
    close_socket(broker_listener);
}

#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
bool tls_fixtures_available() {
    std::ifstream server("tests/fixtures/tls/server.pem");
    return server.good();
}

void tls_write_all(Stream& stream, const std::string& data) {
    std::size_t offset = 0U;
    while (offset < data.size()) {
        const WriteResult result = stream.write(data.data() + offset, data.size() - offset);
        if (result.status == IoStatus::Ok) {
            offset += result.bytes;
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            platform::sleep_ms(1UL);
            continue;
        }
        TEST_FAIL_MESSAGE("tls write failed");
    }
}

std::string tls_read_all(Stream& stream) {
    std::string output;
    char buffer[4096];
    for (;;) {
        const ReadResult result = stream.read(buffer, sizeof(buffer));
        if (result.status == IoStatus::Ok) {
            output.append(buffer, result.bytes);
            continue;
        }
        if (result.status == IoStatus::WouldBlock) {
            platform::sleep_ms(1UL);
            continue;
        }
        if (result.status == IoStatus::Eof) {
            return output;
        }
        TEST_FAIL_MESSAGE("tls read failed");
    }
    return output;
}

void test_tls() {
    if (!tls_fixtures_available()) {
        return;
    }

    DaemonConfig server_config;
    server_config.transport = Transport::Tls;
    server_config.target = "reactor-tls-test";
    server_config.listen_host = "127.0.0.1";
    server_config.listen_port = 0;
    server_config.default_workdir = "/tmp";
    server_config.max_connections = 8U;
    server_config.blocking_workers = 2U;
    server_config.tls_cert_pem = "tests/fixtures/tls/server.pem";
    server_config.tls_key_pem = "tests/fixtures/tls/server.key";
    server_config.tls_ca_pem = "tests/fixtures/tls/ca.pem";
    server_config.tls_pinned_client_cert_pem = "tests/fixtures/tls/client.pem";

    DaemonConfig client_config;
    client_config.reverse_transport = Transport::Tls;
    client_config.reverse_tls_cert_pem = "tests/fixtures/tls/client.pem";
    client_config.reverse_tls_key_pem = "tests/fixtures/tls/client.key";
    client_config.reverse_tls_ca_pem = "tests/fixtures/tls/ca.pem";

    ReactorServer server(server_config);
    std::string start_error;
    TEST_ASSERT(server.start(&start_error));
    std::thread server_thread([&]() { server.run(); });

    const std::shared_ptr<TlsContext> client_context = make_tls_client_context(client_config);
    const int raw_client = connect_local(server.bound_port());
    set_socket_timeout_ms(raw_client, 5000);
    std::shared_ptr<Stream> client =
        make_tls_client_stream(UniqueSocket(raw_client), client_context, "localhost");
    const std::string request = "POST /v1/health HTTP/1.1\r\n"
                                "Host: localhost\r\n"
                                "Connection: close\r\n"
                                "Content-Length: 2\r\n\r\n{}";
    tls_write_all(*client, request);
    const std::string response = tls_read_all(*client);
    TEST_ASSERT(response.find("HTTP/1.1 200") == 0U);
    TEST_ASSERT(response.find("\"status\":\"ok\"") != std::string::npos);
    client->close();

    server.request_stop();
    server_thread.join();
}
#endif

void test_transfer(unsigned short port) {
    const std::string source =
        "/tmp/reactor-export-" + std::to_string(static_cast<long>(getpid())) + ".txt";
    {
        std::ofstream out(source.c_str(), std::ios::binary);
        out << "exported\n";
    }

    std::map<std::string, std::string> export_headers;
    export_headers[server_contract::TRANSFER_STREAM_VERSION_HEADER] =
        server_contract::TRANSFER_STREAM_VERSION_VALUE;
    const Response exported = request_with_headers(
        port,
        "/v1/transfer/export",
        "{\"path\":\"" + source + "\"}",
        export_headers,
        false
    );
    TEST_ASSERT(exported.status == 200);
    const std::string exported_archive = test_tar::decode_framed_transfer_archive(exported.body);
    TEST_ASSERT(exported_archive.find("exported") != std::string::npos);

    const Response info = request(port, "/v1/transfer/path-info", "{\"path\":\"/tmp\"}");
    TEST_ASSERT(info.status == 200);
    TEST_ASSERT(info.body.find("\"exists\":true") != std::string::npos);

    std::string archive;
    test_tar::append_tar_file(archive, ".remote-exec-file", "from-tar\n");
    test_tar::finalize_tar(archive);
    const std::string framed = test_tar::framed_transfer_body(archive);
    const std::string destination =
        "/tmp/reactor-import-" + std::to_string(static_cast<long>(getpid())) + ".txt";

    std::map<std::string, std::string> import_headers;
    import_headers[server_contract::TRANSFER_STREAM_VERSION_HEADER] =
        server_contract::TRANSFER_STREAM_VERSION_VALUE;
    import_headers["content-type"] = server_contract::TRANSFER_STREAM_CONTENT_TYPE;
    import_headers[server_contract::TRANSFER_SOURCE_TYPE_HEADER] = "file";
    import_headers[server_contract::TRANSFER_DESTINATION_PATH_HEADER] =
        base64_encode_bytes(destination);
    import_headers[server_contract::TRANSFER_OVERWRITE_HEADER] = "replace";
    import_headers[server_contract::TRANSFER_CREATE_PARENT_HEADER] = "true";

    const Response imported =
        request_with_headers(port, "/v1/transfer/import", framed, import_headers, true);
    TEST_ASSERT(imported.status == 200);

    std::ifstream in(destination.c_str(), std::ios::binary);
    const std::string content(
        (std::istreambuf_iterator<char>(in)),
        std::istreambuf_iterator<char>()
    );
    TEST_ASSERT(content == "from-tar\n");

    std::remove(source.c_str());
    std::remove(destination.c_str());
}

void test_exec(unsigned short port) {
    const Response start = request(
        port,
        "/v1/exec/start",
        "{\"cmd\":\"echo hello\",\"yield_time_ms\":5000,\"max_output_tokens\":1000}"
    );
    TEST_ASSERT(start.status == 200);
    Json start_body = response_json(start);
    TEST_ASSERT(!start_body.at("running").get<bool>());
    TEST_ASSERT(start_body.at("exit_code").get<int>() == 0);
    TEST_ASSERT(start_body.at("output").get<std::string>().find("hello") != std::string::npos);

    const Response background = request(
        port,
        "/v1/exec/start",
        "{\"cmd\":\"printf start; sleep 1; printf done\",\"yield_time_ms\":100}"
    );
    TEST_ASSERT(background.status == 200);
    Json background_body = response_json(background);
    TEST_ASSERT(background_body.at("running").get<bool>());
    const std::string session_id = background_body.at("daemon_session_id").get<std::string>();
    TEST_ASSERT(!session_id.empty());

    const Response poll = request(
        port,
        "/v1/exec/write",
        "{\"daemon_session_id\":\"" + session_id
            + "\",\"chars\":\"\",\"yield_time_ms\":5000,\"max_output_tokens\":1000}"
    );
    TEST_ASSERT(poll.status == 200);
    Json poll_body = response_json(poll);
    TEST_ASSERT(!poll_body.at("running").get<bool>());
    TEST_ASSERT(poll_body.at("exit_code").get<int>() == 0);
    TEST_ASSERT(poll_body.at("output").get<std::string>().find("done") != std::string::npos);

    const Response unknown = request(
        port,
        "/v1/exec/write",
        "{\"daemon_session_id\":\"sess_missing\",\"chars\":\"\",\"yield_time_ms\":0}"
    );
    TEST_ASSERT(unknown.status == 400);
    TEST_ASSERT(unknown.body.find("unknown_session") != std::string::npos);

    const Response sleep_start =
        request(port, "/v1/exec/start", "{\"cmd\":\"sleep 1\",\"yield_time_ms\":50}");
    TEST_ASSERT(sleep_start.status == 200);
    Json sleep_body = response_json(sleep_start);
    TEST_ASSERT(sleep_body.at("running").get<bool>());
    const std::string sleep_id = sleep_body.at("daemon_session_id").get<std::string>();

    const Response closed = request(
        port,
        "/v1/exec/write",
        "{\"daemon_session_id\":\"" + sleep_id + "\",\"chars\":\"x\",\"yield_time_ms\":0}"
    );
    TEST_ASSERT(closed.status == 400);
    TEST_ASSERT(closed.body.find("stdin_closed") != std::string::npos);

    const Response sleep_poll = request(
        port,
        "/v1/exec/write",
        "{\"daemon_session_id\":\"" + sleep_id + "\",\"chars\":\"\",\"yield_time_ms\":3000}"
    );
    TEST_ASSERT(sleep_poll.status == 200);
    Json sleep_poll_body = response_json(sleep_poll);
    TEST_ASSERT(!sleep_poll_body.at("running").get<bool>());

    const Response tty_start =
        request(port, "/v1/exec/start", "{\"cmd\":\"cat\",\"tty\":true,\"yield_time_ms\":50}");
    TEST_ASSERT(tty_start.status == 200);
    Json tty_body = response_json(tty_start);
    TEST_ASSERT(tty_body.at("running").get<bool>());
    const std::string tty_id = tty_body.at("daemon_session_id").get<std::string>();

    const Response tty_write = request(
        port,
        "/v1/exec/write",
        "{\"daemon_session_id\":\"" + tty_id
            + "\",\"chars\":\"hello\\n\",\"yield_time_ms\":1000,"
              "\"pty_size\":{\"rows\":30,\"cols\":90}}"
    );
    TEST_ASSERT(tty_write.status == 200);
    Json tty_write_body = response_json(tty_write);
    TEST_ASSERT(tty_write_body.at("output").get<std::string>().find("hello") != std::string::npos);
}

void test_reactor_server() {
    DaemonConfig config;
    config.target = "reactor-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = "/tmp";
    config.max_connections = 8U;
    config.blocking_workers = 2U;

    ReactorServer server(config);
    std::string start_error;
    TEST_ASSERT(server.start(&start_error));
    TEST_ASSERT(server.shard_count() == 1U);

    const unsigned short port = server.bound_port();
    TEST_ASSERT(port != 0U);

    std::thread server_thread([&]() { server.run(); });

    const Response health = request(port, "/v1/health", "{}");
    TEST_ASSERT(health.status == 200);
    TEST_ASSERT(health.body.find("\"status\":\"ok\"") != std::string::npos);

    const int keep_alive_fd = connect_local(port);
    {
        const std::string body = "{}";
        const std::string first =
            "POST /v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n" + body;
        send_all_text(keep_alive_fd, first);
        const Response first_response = read_response(keep_alive_fd);
        TEST_ASSERT(first_response.status == 200);

        const std::string second = "POST /v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\n"
                                   "Connection: close\r\nContent-Length: 2\r\n\r\n"
                                   + body;
        send_all_text(keep_alive_fd, second);
        const Response second_response = read_response(keep_alive_fd);
        TEST_ASSERT(second_response.status == 200);
    }
    close_socket(keep_alive_fd);

    const std::string file_name = "reactor-test-file.txt";
    const Response write = request(
        port,
        "/v1/file/write",
        "{\"path\":\"" + file_name + "\",\"content\":\"alpha\\nbeta\\n\",\"max_bytes\":1024}"
    );
    TEST_ASSERT(write.status == 200);

    const Response read = request(
        port,
        "/v1/file/read",
        "{\"path\":\"" + file_name + "\",\"limit\":10,\"max_bytes\":1024}"
    );
    TEST_ASSERT(read.status == 200);
    TEST_ASSERT(read.body.find("alpha") != std::string::npos);
    (void)std::remove("/tmp/reactor-test-file.txt");

    const Response malformed = request(port, "/v1/file/read", "{\"path\":");
    TEST_ASSERT(malformed.status == 400);
    TEST_ASSERT(malformed.body.find("\"bad_request\"") != std::string::npos);

    std::string deep = "{\"path\":";
    for (int i = 0; i < 40; ++i) {
        deep += "[";
    }
    for (int i = 0; i < 40; ++i) {
        deep += "]";
    }
    deep += "}";
    const Response too_deep = request(port, "/v1/file/read", deep);
    TEST_ASSERT(too_deep.status == 400);

    const Response invalid_upgrade = request(port, "/v1/port/tunnel", "{}");
    TEST_ASSERT(invalid_upgrade.status == 400);
    TEST_ASSERT(invalid_upgrade.body.find("bad_request") != std::string::npos);

    test_exec(port);
    test_transfer(port);
    test_tunnel(port);

    server.request_stop();
    server_thread.join();
}
#endif

} // namespace

int main() {
    test_byte_buffer();
    test_json_depth_limit();
#ifndef _WIN32
    test_event_loop();
    test_reactor_server();
    test_reverse();
#ifdef REMOTE_EXEC_CPP_HAS_OPENSSL
    test_tls();
#endif
#endif
    return 0;
}
