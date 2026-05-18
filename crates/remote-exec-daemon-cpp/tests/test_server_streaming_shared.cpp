#include "test_server_streaming_shared.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>

#ifndef _WIN32
#include <poll.h>

#include "posix_eintr.h"
#endif

#include "test_socket_pair.h"

namespace {

std::string stable_test_shell() {
#ifdef _WIN32
    return platform::resolve_default_shell("");
#else
    return platform::resolve_default_shell("/bin/sh");
#endif
}

} // namespace

fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-server-streaming-test";
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

bool wait_until_true(const std::atomic<bool>& value, unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (value.load()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return value.load();
}

void wait_past_resume_timeout(unsigned long resume_timeout_ms) {
    const unsigned long RESUME_TIMEOUT_EXPIRY_MARGIN_MS = 200UL;
    platform::sleep_ms(resume_timeout_ms + RESUME_TIMEOUT_EXPIRY_MARGIN_MS);
}

void initialize_state_with_port_forward_limits(AppState& state,
                                               const fs::path& root,
                                               const PortForwardLimitConfig& limits) {
    state.config = make_server_routes_test_config(root);
    state.config.port_forward_limits = limits;
    state.daemon_instance_id = "test-instance";
    state.hostname = "test-host";
    state.default_shell = stable_test_shell();
    state.port_tunnel_service = create_port_tunnel_service(limits);
}

void initialize_state_with_worker_limit(AppState& state, const fs::path& root, unsigned long max_workers) {
    PortForwardLimitConfig limits;
    limits.max_worker_threads = max_workers;
    initialize_state_with_port_forward_limits(state, root, limits);
}

void initialize_state(AppState& state, const fs::path& root) {
    initialize_state_with_worker_limit(state, root, DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
}

void enable_sandbox(AppState& state) {
    state.sandbox_enabled = state.config.sandbox_configured;
    if (state.sandbox_enabled) {
        state.sandbox = compile_filesystem_sandbox(state.config.sandbox);
    }
}

static std::string socket_label(SOCKET socket) {
    std::ostringstream out;
    out << static_cast<unsigned long long>(socket);
    return out.str();
}

static std::uint64_t deadline_after(unsigned long timeout_ms) {
    return platform::monotonic_ms() + timeout_ms;
}

static unsigned long remaining_timeout_ms(std::uint64_t deadline_ms) {
    const std::uint64_t now = platform::monotonic_ms();
    if (now >= deadline_ms) {
        return 0UL;
    }
    const std::uint64_t remaining = deadline_ms - now;
    return remaining > static_cast<std::uint64_t>(ULONG_MAX) ? ULONG_MAX : static_cast<unsigned long>(remaining);
}

static std::string tunnel_read_failure_message(SOCKET socket,
                                               const char* phase,
                                               const char* detail,
                                               std::size_t offset,
                                               std::size_t size) {
    std::ostringstream out;
    out << "tunnel frame read failed";
    out << " phase=`" << phase << "`";
    out << " socket=" << socket_label(socket);
    out << " detail=`" << detail << "`";
    out << " offset=" << offset;
    out << " size=" << size;
    return out.str();
}

static bool socket_readable_until(SOCKET socket, std::uint64_t deadline_ms) {
    return socket_readable_within(socket, remaining_timeout_ms(deadline_ms));
}

static bool recv_exact_until(SOCKET socket, char* data, std::size_t size, std::uint64_t deadline_ms, const char* phase) {
    std::size_t offset = 0;
    while (offset < size) {
        if (!socket_readable_until(socket, deadline_ms)) {
            TEST_FAIL_MESSAGE(tunnel_read_failure_message(socket, phase, "timeout", offset, size));
        }
        const int received = recv(socket, data + offset, static_cast<int>(size - offset), 0);
        if (received == 0) {
            TEST_FAIL_MESSAGE(tunnel_read_failure_message(socket, phase, "peer disconnected", offset, size));
        }
        if (received < 0) {
            TEST_FAIL_MESSAGE(tunnel_read_failure_message(socket, phase, "recv failed", offset, size));
        }
        offset += static_cast<std::size_t>(received);
    }
    return true;
}

static void recv_exact_or_assert(SOCKET socket, char* data, std::size_t size) {
    recv_exact_until(socket, data, size, deadline_after(kTunnelFrameReadTimeoutMs), "http-head");
}

static uint32_t read_u32_be(const std::vector<unsigned char>& bytes, std::size_t offset) {
    return (static_cast<uint32_t>(bytes[offset]) << 24) | (static_cast<uint32_t>(bytes[offset + 1U]) << 16) |
           (static_cast<uint32_t>(bytes[offset + 2U]) << 8) | static_cast<uint32_t>(bytes[offset + 3U]);
}

static std::string read_http_head_from_socket(SOCKET socket) {
    std::string response;
    while (response.find("\r\n\r\n") == std::string::npos) {
        char ch = '\0';
        recv_exact_or_assert(socket, &ch, 1);
        response.push_back(ch);
    }
    return response;
}

static void send_preface(SOCKET socket) {
    send_all_bytes(socket, port_tunnel_preface(), port_tunnel_preface_size());
}

void send_tunnel_frame(SOCKET socket, const PortTunnelFrame& frame) {
    const std::vector<unsigned char> encoded = encode_port_tunnel_frame(frame);
    send_all_bytes(socket, reinterpret_cast<const char*>(encoded.data()), encoded.size());
}

const char* tunnel_frame_type_name(PortTunnelFrameType type) {
    switch (type) {
    case PortTunnelFrameType::Close:
        return "Close";
    case PortTunnelFrameType::TunnelOpen:
        return "TunnelOpen";
    case PortTunnelFrameType::TunnelReady:
        return "TunnelReady";
    case PortTunnelFrameType::TunnelClosed:
        return "TunnelClosed";
    case PortTunnelFrameType::TunnelClose:
        return "TunnelClose";
    case PortTunnelFrameType::TunnelHeartbeat:
        return "TunnelHeartbeat";
    case PortTunnelFrameType::TunnelHeartbeatAck:
        return "TunnelHeartbeatAck";
    case PortTunnelFrameType::Error:
        return "Error";
    case PortTunnelFrameType::TcpListen:
        return "TcpListen";
    case PortTunnelFrameType::TcpListenOk:
        return "TcpListenOk";
    case PortTunnelFrameType::TcpAccept:
        return "TcpAccept";
    case PortTunnelFrameType::TcpConnect:
        return "TcpConnect";
    case PortTunnelFrameType::TcpConnectOk:
        return "TcpConnectOk";
    case PortTunnelFrameType::TcpData:
        return "TcpData";
    case PortTunnelFrameType::TcpEof:
        return "TcpEof";
    case PortTunnelFrameType::UdpBind:
        return "UdpBind";
    case PortTunnelFrameType::UdpBindOk:
        return "UdpBindOk";
    case PortTunnelFrameType::UdpDatagram:
        return "UdpDatagram";
    case PortTunnelFrameType::SessionOpen:
        return "SessionOpen";
    case PortTunnelFrameType::SessionReady:
        return "SessionReady";
    case PortTunnelFrameType::SessionResume:
        return "SessionResume";
    case PortTunnelFrameType::SessionResumed:
        return "SessionResumed";
    case PortTunnelFrameType::ForwardRecovering:
        return "ForwardRecovering";
    case PortTunnelFrameType::ForwardRecovered:
        return "ForwardRecovered";
    case PortTunnelFrameType::ForwardDrop:
        return "ForwardDrop";
    }
    return "Unknown";
}

PortTunnelFrame read_tunnel_frame_for_phase(SOCKET socket, const char* phase, unsigned long timeout_ms) {
    const std::uint64_t deadline_ms = deadline_after(timeout_ms);
    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    recv_exact_until(socket, reinterpret_cast<char*>(bytes.data()), PORT_TUNNEL_HEADER_LEN, deadline_ms, phase);
    const uint32_t meta_len = read_u32_be(bytes, 8U);
    const uint32_t data_len = read_u32_be(bytes, 12U);
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U) {
        recv_exact_until(socket,
                         reinterpret_cast<char*>(bytes.data() + PORT_TUNNEL_HEADER_LEN),
                         static_cast<std::size_t>(meta_len + data_len),
                         deadline_ms,
                         phase);
    }
    return decode_port_tunnel_frame(bytes);
}

PortTunnelFrame read_tunnel_frame(SOCKET socket) {
    return read_tunnel_frame_for_phase(socket, "read_tunnel_frame", kTunnelFrameReadTimeoutMs);
}

PortTunnelFrame expect_tunnel_frame(SOCKET socket,
                                    PortTunnelFrameType expected_type,
                                    const char* phase,
                                    unsigned long timeout_ms) {
    const PortTunnelFrame frame = read_tunnel_frame_for_phase(socket, phase, timeout_ms);
    if (frame.type != expected_type) {
        std::ostringstream out;
        out << "unexpected tunnel frame";
        out << " phase=`" << phase << "`";
        out << " socket=" << socket_label(socket);
        out << " expected=" << tunnel_frame_type_name(expected_type);
        out << " actual=" << tunnel_frame_type_name(frame.type);
        out << " stream_id=" << frame.stream_id;
        TEST_FAIL_MESSAGE(out.str());
    }
    return frame;
}

bool try_read_tunnel_frame_with_timeout(SOCKET socket, unsigned long timeout_ms, PortTunnelFrame* frame) {
#ifdef _WIN32
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(socket, &read_fds);
    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(socket + 1, &read_fds, NULL, NULL, &timeout);
#else
    struct pollfd descriptor;
    descriptor.fd = socket;
    descriptor.events = POLLIN;
    descriptor.revents = 0;
    const int ready = posix_eintr::poll_for_ms(&descriptor, 1, timeout_ms);
#endif
    TEST_ASSERT(ready >= 0);
    if (ready == 0) {
        return false;
    }
    *frame = read_tunnel_frame_for_phase(socket, "try_read_tunnel_frame_with_timeout", timeout_ms);
    return true;
}

bool socket_readable_within(SOCKET socket, unsigned long timeout_ms) {
#ifdef _WIN32
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(socket, &read_fds);
    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(socket + 1, &read_fds, NULL, NULL, &timeout);
    TEST_ASSERT(ready >= 0);
    return ready > 0 && FD_ISSET(socket, &read_fds);
#else
    struct pollfd descriptor;
    descriptor.fd = socket;
    descriptor.events = POLLIN;
    descriptor.revents = 0;
    const int ready = posix_eintr::poll_for_ms(&descriptor, 1, timeout_ms);
    TEST_ASSERT(ready >= 0);
    return ready > 0 && (descriptor.revents & POLLIN) != 0;
#endif
}

void assert_socket_closed_within(SOCKET socket, unsigned long timeout_ms) {
    TEST_ASSERT(socket_readable_within(socket, timeout_ms));
    char byte = '\0';
    const int received = recv(socket, &byte, 1, 0);
    TEST_ASSERT(received <= 0);
}

bool tcp_listener_has_pending_connection(SOCKET socket, unsigned long timeout_ms) {
    return socket_readable_within(socket, timeout_ms);
}

void assert_tunnel_error_code(const PortTunnelFrame& frame, const std::string& code) {
    TEST_ASSERT(frame.type == PortTunnelFrameType::Error);
    const Json meta = Json::parse(frame.meta);
    TEST_ASSERT(meta.at("code").get<std::string>() == code);
}

void assert_forward_drop(const PortTunnelFrame& frame, const std::string& kind, const std::string& reason) {
    TEST_ASSERT(frame.type == PortTunnelFrameType::ForwardDrop);
    const Json meta = Json::parse(frame.meta);
    TEST_ASSERT(meta.at("kind").get<std::string>() == kind);
    TEST_ASSERT(meta.at("count").get<unsigned long>() == 1UL);
    TEST_ASSERT(meta.at("reason").get<std::string>() == reason);
}

static std::thread start_server_thread(AppState& state, UniqueSocket* server_socket) {
    return std::thread(
        [&state](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(state, std::move(owned_socket));
        },
        server_socket->release());
}

void open_tunnel(AppState& state, UniqueSocket* client_socket, std::thread* server_thread) {
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    client_socket->reset(sockets.second.release());
    *server_thread = start_server_thread(state, &server_socket);

    send_all(client_socket->get(),
             "POST /v1/port/tunnel HTTP/1.1\r\n"
             "Connection: Upgrade\r\n"
             "Upgrade: remote-exec-port-tunnel\r\n"
             "X-Remote-Exec-Port-Tunnel-Version: 4\r\n"
             "X-Request-Id: cpp-tunnel-req\r\n"
             "\r\n");
    const std::string response = read_http_head_from_socket(client_socket->get());
    TEST_ASSERT(response.find("HTTP/1.1 101 Switching Protocols\r\n") == 0);
    TEST_ASSERT(response.find("Connection: Upgrade\r\n") != std::string::npos);
    TEST_ASSERT(response.find("Upgrade: remote-exec-port-tunnel\r\n") != std::string::npos);
    TEST_ASSERT(response.find("x-request-id: cpp-tunnel-req\r\n") != std::string::npos);
    send_preface(client_socket->get());
}

PortTunnelFrame json_frame(PortTunnelFrameType type, uint32_t stream_id, const Json& meta) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta = meta.dump();
    return frame;
}

PortTunnelFrame data_frame(PortTunnelFrameType type, uint32_t stream_id, const std::vector<unsigned char>& data) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.data = data;
    return frame;
}

PortTunnelFrame empty_frame(PortTunnelFrameType type, uint32_t stream_id) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    return frame;
}

Json tunnel_open_meta(const std::string& role,
                      const std::string& protocol,
                      uint64_t generation,
                      const std::string& resume_session_id) {
    Json meta{{"forward_id", "fwd_cpp_test"},
              {"role", role},
              {"side", "cpp-test"},
              {"generation", generation},
              {"protocol", protocol}};
    if (!resume_session_id.empty()) {
        meta["resume_session_id"] = resume_session_id;
    }
    return meta;
}

PortTunnelFrame open_v4_tunnel(AppState& state,
                               UniqueSocket* client_socket,
                               std::thread* server_thread,
                               const std::string& role,
                               const std::string& protocol,
                               uint64_t generation,
                               const std::string& resume_session_id) {
    open_tunnel(state, client_socket, server_thread);
    send_tunnel_frame(client_socket->get(),
                      json_frame(PortTunnelFrameType::TunnelOpen,
                                 0U,
                                 tunnel_open_meta(role, protocol, generation, resume_session_id)));
    return expect_tunnel_frame(client_socket->get(), PortTunnelFrameType::TunnelReady, "open_v4_tunnel ready");
}

void close_tunnel(UniqueSocket* client_socket, std::thread* server_thread) {
    client_socket->reset();
    server_thread->join();
}

void wait_until_bindable(const std::string& endpoint) {
    for (int attempt = 0; attempt < 40; ++attempt) {
        try {
            UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
            if (rebound.valid()) {
                return;
            }
        } catch (const std::exception&) {
        }
        platform::sleep_ms(25UL);
    }
    std::fprintf(stderr, "endpoint `%s` did not become bindable\n", endpoint.c_str());
    std::abort();
}
