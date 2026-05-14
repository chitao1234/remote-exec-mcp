#include "test_server_streaming_shared.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>

#include <netdb.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/time.h>

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
    state.default_shell = platform::resolve_default_shell("");
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

static void recv_exact_or_assert(SOCKET socket, char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int received = recv(socket, data + offset, static_cast<int>(size - offset), 0);
        TEST_ASSERT(received > 0);
        offset += static_cast<std::size_t>(received);
    }
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

PortTunnelFrame read_tunnel_frame(SOCKET socket) {
    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    recv_exact_or_assert(socket, reinterpret_cast<char*>(bytes.data()), PORT_TUNNEL_HEADER_LEN);
    const uint32_t meta_len = read_u32_be(bytes, 8U);
    const uint32_t data_len = read_u32_be(bytes, 12U);
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U) {
        recv_exact_or_assert(socket,
                             reinterpret_cast<char*>(bytes.data() + PORT_TUNNEL_HEADER_LEN),
                             static_cast<std::size_t>(meta_len + data_len));
    }
    return decode_port_tunnel_frame(bytes);
}

bool try_read_tunnel_frame_with_timeout(SOCKET socket, unsigned long timeout_ms, PortTunnelFrame* frame) {
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(socket, &read_fds);
    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(socket + 1, &read_fds, NULL, NULL, &timeout);
    TEST_ASSERT(ready >= 0);
    if (ready == 0) {
        return false;
    }
    *frame = read_tunnel_frame(socket);
    return true;
}

bool tcp_listener_has_pending_connection(SOCKET socket, unsigned long timeout_ms) {
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(socket, &read_fds);
    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(socket + 1, &read_fds, NULL, NULL, &timeout);
    TEST_ASSERT(ready >= 0);
    return ready > 0 && FD_ISSET(socket, &read_fds);
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
    int sockets[2];
    TEST_ASSERT(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    client_socket->reset(sockets[1]);
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
    const PortTunnelFrame ready = read_tunnel_frame(client_socket->get());
    TEST_ASSERT(ready.type == PortTunnelFrameType::TunnelReady);
    return ready;
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
