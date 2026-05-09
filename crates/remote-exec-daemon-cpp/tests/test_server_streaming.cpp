#include <algorithm>
#include <cassert>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <thread>

#include <netdb.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/time.h>

#include "config.h"
#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "path_policy.h"
#include "platform.h"
#include "port_forward_endpoint.h"
#include "port_forward_socket_ops.h"
#include "port_tunnel.h"
#include "port_tunnel_frame.h"
#include "process_session.h"
#include "server.h"
#include "server_transport.h"

namespace fs = std::filesystem;

static fs::path make_test_root() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-server-streaming-test";
    fs::remove_all(root);
    fs::create_directories(root);
    return root;
}

static DaemonConfig make_config(const fs::path& root) {
    DaemonConfig config;
    config.target = "cpp-test";
    config.listen_host = "127.0.0.1";
    config.listen_port = 0;
    config.default_workdir = root.string();
    config.default_shell.clear();
    config.allow_login_shell = true;
    config.http_auth_bearer_token.clear();
    config.max_request_header_bytes = 65536;
    config.max_request_body_bytes = 536870912;
    config.max_open_sessions = 64;
    config.port_forward_max_worker_threads = DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS;
    config.port_forward_limits = default_port_forward_limit_config();
    config.yield_time = default_yield_time_config();
    return config;
}

static void initialize_state_with_port_forward_limits(
    AppState& state,
    const fs::path& root,
    const PortForwardLimitConfig& limits
) {
    state.config = make_config(root);
    state.config.port_forward_limits = limits;
    state.config.port_forward_max_worker_threads = limits.max_worker_threads;
    state.daemon_instance_id = "test-instance";
    state.hostname = "test-host";
    state.default_shell = platform::resolve_default_shell("");
    state.port_tunnel_service = create_port_tunnel_service(limits);
}

static void initialize_state_with_worker_limit(
    AppState& state,
    const fs::path& root,
    unsigned long max_workers
) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_worker_threads = max_workers;
    initialize_state_with_port_forward_limits(state, root, limits);
}

static void initialize_state(AppState& state, const fs::path& root) {
    initialize_state_with_worker_limit(
        state,
        root,
        DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS
    );
}

static void enable_sandbox(AppState& state) {
    state.sandbox_enabled = state.config.sandbox_configured;
    if (state.sandbox_enabled) {
        state.sandbox = compile_filesystem_sandbox(host_path_policy(), state.config.sandbox);
    }
}

static std::string read_all_from_socket(SOCKET socket) {
    std::string output;
    char buffer[4096];
    for (;;) {
        const int received = recv(socket, buffer, sizeof(buffer), 0);
        if (received <= 0) {
            break;
        }
        output.append(buffer, static_cast<std::size_t>(received));
    }
    return output;
}

static void recv_exact_or_assert(SOCKET socket, char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int received = recv(
            socket,
            data + offset,
            static_cast<int>(size - offset),
            0
        );
        assert(received > 0);
        offset += static_cast<std::size_t>(received);
    }
}

static uint32_t read_u32_be(const std::vector<unsigned char>& bytes, std::size_t offset) {
    return (static_cast<uint32_t>(bytes[offset]) << 24) |
           (static_cast<uint32_t>(bytes[offset + 1U]) << 16) |
           (static_cast<uint32_t>(bytes[offset + 2U]) << 8) |
           static_cast<uint32_t>(bytes[offset + 3U]);
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

static void send_tunnel_frame(SOCKET socket, const PortTunnelFrame& frame) {
    const std::vector<unsigned char> encoded = encode_port_tunnel_frame(frame);
    send_all_bytes(
        socket,
        reinterpret_cast<const char*>(encoded.data()),
        encoded.size()
    );
}

static PortTunnelFrame read_tunnel_frame(SOCKET socket) {
    std::vector<unsigned char> bytes(PORT_TUNNEL_HEADER_LEN, 0U);
    recv_exact_or_assert(
        socket,
        reinterpret_cast<char*>(bytes.data()),
        PORT_TUNNEL_HEADER_LEN
    );
    const uint32_t meta_len = read_u32_be(bytes, 8U);
    const uint32_t data_len = read_u32_be(bytes, 12U);
    bytes.resize(PORT_TUNNEL_HEADER_LEN + meta_len + data_len);
    if (meta_len + data_len > 0U) {
        recv_exact_or_assert(
            socket,
            reinterpret_cast<char*>(bytes.data() + PORT_TUNNEL_HEADER_LEN),
            static_cast<std::size_t>(meta_len + data_len)
        );
    }
    return decode_port_tunnel_frame(bytes);
}

static bool try_read_tunnel_frame_with_timeout(
    SOCKET socket,
    unsigned long timeout_ms,
    PortTunnelFrame* frame
) {
    fd_set read_fds;
    FD_ZERO(&read_fds);
    FD_SET(socket, &read_fds);
    timeval timeout;
    timeout.tv_sec = static_cast<long>(timeout_ms / 1000UL);
    timeout.tv_usec = static_cast<long>((timeout_ms % 1000UL) * 1000UL);
    const int ready = select(socket + 1, &read_fds, NULL, NULL, &timeout);
    assert(ready >= 0);
    if (ready == 0) {
        return false;
    }
    *frame = read_tunnel_frame(socket);
    return true;
}

static void assert_tunnel_error_code(
    const PortTunnelFrame& frame,
    const std::string& code
) {
    assert(frame.type == PortTunnelFrameType::Error);
    const Json meta = Json::parse(frame.meta);
    assert(meta.at("code").get<std::string>() == code);
}

static void assert_forward_drop(
    const PortTunnelFrame& frame,
    const std::string& kind,
    const std::string& reason
) {
    assert(frame.type == PortTunnelFrameType::ForwardDrop);
    const Json meta = Json::parse(frame.meta);
    assert(meta.at("kind").get<std::string>() == kind);
    assert(meta.at("count").get<unsigned long>() == 1UL);
    assert(meta.at("reason").get<std::string>() == reason);
}

static std::size_t response_content_length(const std::string& header_block) {
    const std::string marker = "\r\nContent-Length: ";
    const std::size_t start = header_block.find(marker);
    assert(start != std::string::npos);
    const std::size_t value_start = start + marker.size();
    const std::size_t value_end = header_block.find("\r\n", value_start);
    assert(value_end != std::string::npos);
    return static_cast<std::size_t>(
        std::strtoull(header_block.substr(value_start, value_end - value_start).c_str(), NULL, 10)
    );
}

static std::string read_content_length_response_from_socket(SOCKET socket) {
    std::string response;
    while (response.find("\r\n\r\n") == std::string::npos) {
        char ch = '\0';
        recv_exact_or_assert(socket, &ch, 1);
        response.push_back(ch);
    }

    const std::size_t header_end = response.find("\r\n\r\n");
    const std::size_t content_length = response_content_length(response.substr(0, header_end));
    const std::size_t total_size = header_end + 4U + content_length;
    while (response.size() < total_size) {
        char buffer[4096];
        const std::size_t remaining = total_size - response.size();
        const std::size_t request_size = std::min<std::size_t>(remaining, sizeof(buffer));
        recv_exact_or_assert(socket, buffer, request_size);
        response.append(buffer, request_size);
    }

    return response;
}

static std::string read_text_file(const fs::path& path) {
    std::ifstream input(path.c_str(), std::ios::binary);
    return std::string((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
}

static void write_text_file(const fs::path& path, const std::string& value) {
    std::ofstream output(path.c_str(), std::ios::binary | std::ios::trunc);
    output << value;
}

static void send_request_and_close_writer(SOCKET socket, const std::string& request) {
    send_all(socket, request);
    shutdown(socket, SHUT_WR);
}

static std::string response_body(const std::string& response) {
    const std::size_t header_end = response.find("\r\n\r\n");
    assert(header_end != std::string::npos);
    return response.substr(header_end + 4);
}

static std::string decode_chunked_response_body(const std::string& response) {
    const std::string body = response_body(response);
    std::string decoded;
    std::size_t offset = 0;
    for (;;) {
        const std::size_t line_end = body.find("\r\n", offset);
        assert(line_end != std::string::npos);
        std::size_t chunk_size = 0;
        std::istringstream size_stream(body.substr(offset, line_end - offset));
        size_stream >> std::hex >> chunk_size;
        offset = line_end + 2;
        if (chunk_size == 0U) {
            assert(body.compare(offset, 2, "\r\n") == 0);
            return decoded;
        }
        assert(offset + chunk_size + 2 <= body.size());
        decoded.append(body, offset, chunk_size);
        offset += chunk_size;
        assert(body.compare(offset, 2, "\r\n") == 0);
        offset += 2;
    }
}

static std::string octal_field(std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer,
        sizeof(buffer),
        "%0*llo",
        static_cast<int>(width - 1),
        static_cast<unsigned long long>(value)
    );
    std::string field(width, '\0');
    const std::string digits(buffer);
    const std::size_t used = std::min(width - 1, digits.size());
    field.replace(width - 1 - used, used, digits.substr(digits.size() - used));
    field[width - 1] = ' ';
    return field;
}

static std::uint64_t parse_octal_value(const char* data, std::size_t size) {
    std::size_t index = 0;
    while (index < size && (data[index] == ' ' || data[index] == '\0')) {
        ++index;
    }
    std::uint64_t value = 0;
    while (index < size && data[index] >= '0' && data[index] <= '7') {
        value = (value * 8) + static_cast<std::uint64_t>(data[index] - '0');
        ++index;
    }
    return value;
}

static std::string single_file_tar_body(const std::string& archive) {
    assert(archive.size() >= 512);
    const char* header = archive.data();
    std::size_t path_length = 0;
    while (path_length < 100 && header[path_length] != '\0') {
        ++path_length;
    }
    assert(std::string(header, path_length) == ".remote-exec-file");
    assert(header[156] == '0');
    const std::uint64_t size = parse_octal_value(header + 124, 12);
    assert(512 + static_cast<std::size_t>(size) <= archive.size());
    return archive.substr(512, static_cast<std::size_t>(size));
}

static void set_bytes(std::string* header, std::size_t offset, std::size_t width, const std::string& value) {
    header->replace(offset, std::min(width, value.size()), value.substr(0, width));
}

static void write_checksum(std::string* header) {
    std::fill(header->begin() + 148, header->begin() + 156, ' ');
    unsigned int checksum = 0;
    for (std::size_t i = 0; i < header->size(); ++i) {
        checksum += static_cast<unsigned char>((*header)[i]);
    }
    header->replace(148, 8, octal_field(8, checksum));
}

static std::string tar_with_single_file(const std::string& body) {
    std::string archive;
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, ".remote-exec-file");
    header.replace(100, 8, octal_field(8, 0644));
    header.replace(108, 8, octal_field(8, 0));
    header.replace(116, 8, octal_field(8, 0));
    header.replace(124, 12, octal_field(12, body.size()));
    header.replace(136, 12, octal_field(12, 0));
    header[156] = '0';
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);
    archive.append(header);
    archive.append(body);
    const std::size_t remainder = body.size() % 512;
    if (remainder != 0) {
        archive.append(512 - remainder, '\0');
    }
    archive.append(1024, '\0');
    return archive;
}

static std::string chunked_body(const std::string& body) {
    std::ostringstream out;
    std::size_t offset = 0;
    while (offset < body.size()) {
        const std::size_t len = std::min<std::size_t>(37, body.size() - offset);
        out << std::hex << len << "\r\n";
        out.write(body.data() + offset, static_cast<std::streamsize>(len));
        out << "\r\n";
        offset += len;
    }
    out << "0\r\n\r\n";
    return out.str();
}

static std::string run_single_request(AppState& state, const std::string& request) {
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    UniqueSocket client_socket(sockets[1]);
    send_request_and_close_writer(client_socket.get(), request);
    handle_client(state, std::move(server_socket));
    return read_all_from_socket(client_socket.get());
}

static std::string json_post_request(const std::string& path, const Json& body) {
    const std::string payload = body.dump();
    std::ostringstream request;
    request << "POST " << path << " HTTP/1.1\r\n"
            << "Content-Length: " << payload.size() << "\r\n"
            << "\r\n"
            << payload;
    return request.str();
}

static std::string json_post_request_with_extra_headers(
    const std::string& path,
    const Json& body,
    const std::string& extra_headers
) {
    const std::string payload = body.dump();
    std::ostringstream request;
    request << "POST " << path << " HTTP/1.1\r\n"
            << "Content-Length: " << payload.size() << "\r\n"
            << extra_headers
            << "\r\n"
            << payload;
    return request.str();
}

static void assert_persistent_json_requests_reuse_socket(AppState& state) {
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    UniqueSocket client_socket(sockets[1]);
    std::thread server_thread(
        [&state](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(state, std::move(owned_socket));
        },
        server_socket.release()
    );

    send_all(client_socket.get(), json_post_request("/v1/health", Json::object()));
    const std::string first_response =
        read_content_length_response_from_socket(client_socket.get());
    assert(first_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(first_response.find("Connection: close\r\n") == std::string::npos);
    assert(Json::parse(response_body(first_response)).at("status").get<std::string>() == "ok");

    send_all(
        client_socket.get(),
        json_post_request_with_extra_headers(
            "/v1/target-info",
            Json::object(),
            "Connection: close\r\n"
        )
    );
    const std::string second_response =
        read_content_length_response_from_socket(client_socket.get());
    assert(second_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(second_response.find("Connection: close\r\n") == std::string::npos);
    assert(Json::parse(response_body(second_response)).at("target").get<std::string>() == "cpp-test");

    char extra = '\0';
    assert(recv(client_socket.get(), &extra, 1, 0) == 0);
    server_thread.join();
}

static std::thread start_server_thread(AppState& state, UniqueSocket* server_socket) {
    return std::thread(
        [&state](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(state, std::move(owned_socket));
        },
        server_socket->release()
    );
}

static void open_tunnel(AppState& state, UniqueSocket* client_socket, std::thread* server_thread) {
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    client_socket->reset(sockets[1]);
    *server_thread = start_server_thread(state, &server_socket);

    send_all(
        client_socket->get(),
        "POST /v1/port/tunnel HTTP/1.1\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: remote-exec-port-tunnel\r\n"
        "X-Remote-Exec-Port-Tunnel-Version: 4\r\n"
        "\r\n"
    );
    const std::string response = read_http_head_from_socket(client_socket->get());
    assert(response.find("HTTP/1.1 101 Switching Protocols\r\n") == 0);
    assert(response.find("Connection: Upgrade\r\n") != std::string::npos);
    assert(response.find("Upgrade: remote-exec-port-tunnel\r\n") != std::string::npos);
    send_preface(client_socket->get());
}

static PortTunnelFrame json_frame(
    PortTunnelFrameType type,
    uint32_t stream_id,
    const Json& meta
) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.meta = meta.dump();
    return frame;
}

static PortTunnelFrame data_frame(
    PortTunnelFrameType type,
    uint32_t stream_id,
    const std::vector<unsigned char>& data
) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    frame.data = data;
    return frame;
}

static PortTunnelFrame empty_frame(PortTunnelFrameType type, uint32_t stream_id) {
    PortTunnelFrame frame;
    frame.type = type;
    frame.flags = 0U;
    frame.stream_id = stream_id;
    return frame;
}

static Json tunnel_open_meta(
    const std::string& role,
    const std::string& protocol,
    uint64_t generation,
    const std::string& resume_session_id = std::string()
) {
    Json meta{
        {"forward_id", "fwd_cpp_test"},
        {"role", role},
        {"side", "cpp-test"},
        {"generation", generation},
        {"protocol", protocol}
    };
    if (!resume_session_id.empty()) {
        meta["resume_session_id"] = resume_session_id;
    }
    return meta;
}

static PortTunnelFrame open_v4_tunnel(
    AppState& state,
    UniqueSocket* client_socket,
    std::thread* server_thread,
    const std::string& role,
    const std::string& protocol,
    uint64_t generation,
    const std::string& resume_session_id = std::string()
) {
    open_tunnel(state, client_socket, server_thread);
    send_tunnel_frame(
        client_socket->get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta(role, protocol, generation, resume_session_id)
        )
    );
    const PortTunnelFrame ready = read_tunnel_frame(client_socket->get());
    assert(ready.type == PortTunnelFrameType::TunnelReady);
    return ready;
}

static void close_tunnel(UniqueSocket* client_socket, std::thread* server_thread) {
    client_socket->reset();
    server_thread->join();
}

static void wait_until_bindable(const std::string& endpoint) {
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

static void assert_tunnel_close_releases_tcp_listener(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", "operator_close"}
            }
        )
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelClosed);
    close_tunnel(&client_socket, &server_thread);

    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    assert(rebound.valid());
}

static void assert_terminal_tunnel_error_releases_tcp_listener_immediately(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    const unsigned char invalid_frame[PORT_TUNNEL_HEADER_LEN] = {
        static_cast<unsigned char>(PortTunnelFrameType::TcpData),
        0U,
        1U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
        0U,
    };
    send_all_bytes(
        client_socket.get(),
        reinterpret_cast<const char*>(invalid_frame),
        sizeof(invalid_frame)
    );

    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    assert(error.type == PortTunnelFrameType::Error);
    assert(error.stream_id == 0U);
    const Json error_meta = Json::parse(error.meta);
    assert(error_meta.at("code").get<std::string>() == "invalid_port_tunnel");
    assert(error_meta.at("fatal").get<bool>());

    close_tunnel(&client_socket, &server_thread);
    wait_until_bindable(endpoint);
}

static void assert_tunnel_open_ready_and_close_round_trip(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL)
        )
    );
    const PortTunnelFrame ready = read_tunnel_frame(client_socket.get());
    assert(ready.type == PortTunnelFrameType::TunnelReady);
    const Json ready_meta = Json::parse(ready.meta);
    assert(ready_meta.at("generation").get<uint64_t>() == 1ULL);
    assert(ready_meta.at("session_id").get<std::string>().find("sess_cpp_") == 0);
    assert(ready_meta.at("resume_timeout_ms").get<unsigned long>() > 0UL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", "operator_close"}
            }
        )
    );
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    assert(closed.type == PortTunnelFrameType::TunnelClosed);
    assert(closed.stream_id == 0U);
    assert(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_close_releases_retained_listener_immediately(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL)
        )
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelReady);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", "operator_close"}
            }
        )
    );
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    assert(closed.type == PortTunnelFrameType::TunnelClosed);
    assert(closed.stream_id == 0U);

    close_tunnel(&client_socket, &server_thread);
    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    assert(rebound.valid());
}

static void assert_port_tunnel_worker_limit_is_reported(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 1UL);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL)
        )
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelReady);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpListenOk);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 3U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    assert(error.type == PortTunnelFrameType::Error);
    assert(error.stream_id == 3U);
    const Json error_meta = Json::parse(error.meta);
    assert(error_meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    assert(error_meta.at("message").get<std::string>() == "port tunnel worker limit reached");
    assert(!error_meta.at("fatal").get<bool>());

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tcp_connect_worker_limit_errors_before_success(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 1UL);

    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    UniqueSocket worker_holder;
    std::thread worker_holder_thread;
    open_v4_tunnel(state, &worker_holder, &worker_holder_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(
        worker_holder.get(),
        json_frame(PortTunnelFrameType::TcpListen, 99U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert(read_tunnel_frame(worker_holder.get()).type == PortTunnelFrameType::TcpListenOk);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}})
    );
    const PortTunnelFrame response = read_tunnel_frame(client_socket.get());
    assert(response.type == PortTunnelFrameType::Error);
    assert(response.stream_id == 1U);
    const Json meta = Json::parse(response.meta);
    assert(meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    assert(meta.at("message").get<std::string>() == "port tunnel worker limit reached");
    assert(!meta.at("fatal").get<bool>());

    close_tunnel(&worker_holder, &worker_holder_thread);
    close_tunnel(&client_socket, &server_thread);
}

static void assert_tcp_connect_read_thread_failure_errors_before_success(const fs::path& root) {
    AppState state;
    initialize_state(state, root);

    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    set_forced_tcp_read_thread_failures(1UL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}})
    );
    const PortTunnelFrame response = read_tunnel_frame(client_socket.get());
    assert(response.type == PortTunnelFrameType::Error);
    assert(response.stream_id == 1U);
    const Json meta = Json::parse(response.meta);
    assert(meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    assert(meta.at("message").get<std::string>() == "port tunnel worker limit reached");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tcp_accept_read_thread_failure_drops_before_accept(const fs::path& root) {
    AppState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    set_forced_tcp_read_thread_failures(1UL);
    UniqueSocket dropped_peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame unexpected;
    assert(!try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &unexpected));

    UniqueSocket accepted_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    assert(accepted.type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_accept_read_thread_failure_drops_before_accept(const fs::path& root) {
    AppState state;
    initialize_state(state, root);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    set_forced_tcp_read_thread_failures(1UL);
    UniqueSocket dropped_peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame unexpected;
    assert(!try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &unexpected));

    UniqueSocket accepted_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    assert(accepted.type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_accept_worker_pressure_is_local_drop(const fs::path& root) {
    AppState state;
    initialize_state_with_worker_limit(state, root, 1UL);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame drop_report;
    assert(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "tcp_stream", "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_retained_tcp_accept_pressure_is_local_drop(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_worker_threads = 3UL;
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string hold_endpoint = socket_local_endpoint(listener.get());

    UniqueSocket connect_client;
    std::thread connect_thread;
    open_v4_tunnel(state, &connect_client, &connect_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(
        connect_client.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 3U, Json{{"endpoint", hold_endpoint}})
    );
    assert(read_tunnel_frame(connect_client.get()).type == PortTunnelFrameType::TcpConnectOk);
    UniqueSocket held_peer(accept(listener.get(), NULL, NULL));
    assert(held_peer.valid());

    UniqueSocket refused_peer(connect_port_forward_socket(endpoint, "tcp"));
    assert(refused_peer.valid());
    PortTunnelFrame drop_report;
    assert(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "tcp_stream", "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
    close_tunnel(&connect_client, &connect_thread);
}

static void assert_tunnel_ready_reports_configured_limits(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_worker_threads = 7UL;
    limits.max_retained_sessions = 2UL;
    limits.max_retained_listeners = 4UL;
    limits.max_udp_binds = 5UL;
    limits.max_active_tcp_streams = 3UL;
    limits.max_tunnel_queued_bytes = 4096UL;
    limits.tunnel_io_timeout_ms = 6000UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL)
        )
    );
    const PortTunnelFrame ready = read_tunnel_frame(client_socket.get());
    assert(ready.type == PortTunnelFrameType::TunnelReady);
    const Json ready_meta = Json::parse(ready.meta);
    const Json ready_limits = ready_meta.at("limits");
    assert(ready_limits.at("max_active_tcp_streams").get<unsigned long>() == 3UL);
    assert(ready_limits.at("max_udp_peers").get<unsigned long>() == 5UL);
    assert(ready_limits.at("max_queued_bytes").get<unsigned long>() == 4096UL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_rejects_data_plane_before_open(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", "127.0.0.1:1"}})
    );
    PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 1U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 3U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 3U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_rejects_frames_for_wrong_role_or_protocol(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 1U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 3U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 3U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("connect", "tcp", 2ULL)
        )
    );
    error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 0U);
    assert_tunnel_error_code(error, "port_tunnel_already_attached");

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 5U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 5U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_legacy_session_frames_are_reserved_but_unsupported(AppState& state) {
    const PortTunnelFrameType legacy_frames[] = {
        PortTunnelFrameType::SessionOpen,
        PortTunnelFrameType::SessionResume
    };
    for (std::size_t i = 0U; i < sizeof(legacy_frames) / sizeof(legacy_frames[0]); ++i) {
        UniqueSocket client_socket;
        std::thread server_thread;
        open_tunnel(state, &client_socket, &server_thread);

        send_tunnel_frame(
            client_socket.get(),
            json_frame(legacy_frames[i], 0U, Json{{"session_id", "legacy_session"}})
        );
        assert_tunnel_error_code(
            read_tunnel_frame(client_socket.get()),
            "invalid_port_tunnel"
        );

        close_tunnel(&client_socket, &server_thread);
    }
}

static void assert_retained_session_limit_is_enforced(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_retained_sessions = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket first_client;
    std::thread first_thread;
    open_v4_tunnel(state, &first_client, &first_thread, "listen", "tcp", 1ULL);

    UniqueSocket second_client;
    std::thread second_thread;
    open_tunnel(state, &second_client, &second_thread);
    send_tunnel_frame(
        second_client.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL)
        )
    );
    assert_tunnel_error_code(
        read_tunnel_frame(second_client.get()),
        "port_tunnel_limit_exceeded"
    );

    close_tunnel(&second_client, &second_thread);
    close_tunnel(&first_client, &first_thread);
}

static void assert_retained_listener_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_retained_sessions = 2UL;
    limits.max_retained_listeners = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpListenOk);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 3U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert_tunnel_error_code(
        read_tunnel_frame(client_socket.get()),
        "port_tunnel_limit_exceeded"
    );

    send_tunnel_frame(
        client_socket.get(),
        empty_frame(PortTunnelFrameType::Close, 1U)
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpListenOk);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_udp_bind_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_udp_binds = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "udp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::UdpBindOk);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 3U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert_tunnel_error_code(
        read_tunnel_frame(client_socket.get()),
        "port_tunnel_limit_exceeded"
    );

    send_tunnel_frame(
        client_socket.get(),
        empty_frame(PortTunnelFrameType::Close, 1U)
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 5U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::UdpBindOk);

    close_tunnel(&client_socket, &server_thread);
}

static std::thread accept_and_hold_tcp_connections(SOCKET listener_socket, int count) {
    return std::thread([listener_socket, count]() {
        std::vector<UniqueSocket> accepted;
        for (int index = 0; index < count; ++index) {
            const SOCKET socket = accept(listener_socket, NULL, NULL);
            assert(socket != INVALID_SOCKET);
            accepted.push_back(UniqueSocket(socket));
        }
        platform::sleep_ms(100UL);
    });
}

static std::thread accept_and_send_tcp_payload(
    SOCKET listener_socket,
    const std::vector<unsigned char>& payload
) {
    return std::thread([listener_socket, payload]() {
        UniqueSocket accepted(accept(listener_socket, NULL, NULL));
        assert(accepted.valid());
        send_all_bytes(
            accepted.get(),
            reinterpret_cast<const char*>(payload.data()),
            payload.size()
        );
        platform::sleep_ms(100UL);
    });
}

static std::string closed_loopback_tcp_endpoint() {
    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());
    listener.reset();
    return endpoint;
}

static void assert_active_tcp_stream_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket echo_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(echo_listener.get());
    std::thread accept_thread = accept_and_hold_tcp_connections(echo_listener.get(), 2);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TcpConnect,
            7U,
            Json{{"endpoint", closed_loopback_tcp_endpoint()}}
        )
    );
    assert_tunnel_error_code(read_tunnel_frame(client_socket.get()), "port_connect_failed");

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 3U, Json{{"endpoint", endpoint}})
    );
    assert_tunnel_error_code(
        read_tunnel_frame(client_socket.get()),
        "port_tunnel_limit_exceeded"
    );

    send_tunnel_frame(
        client_socket.get(),
        empty_frame(PortTunnelFrameType::Close, 1U)
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 5U, Json{{"endpoint", endpoint}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);

    close_tunnel(&client_socket, &server_thread);
    echo_listener.reset();
    accept_thread.join();
}

static void assert_active_tcp_accept_limit_is_enforced_and_released(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_active_tcp_streams = 1UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket first_peer(connect_port_forward_socket(endpoint, "tcp"));
    const PortTunnelFrame first_accept = read_tunnel_frame(client_socket.get());
    assert(first_accept.type == PortTunnelFrameType::TcpAccept);

    UniqueSocket refused_peer(connect_port_forward_socket(endpoint, "tcp"));
    PortTunnelFrame drop_report;
    assert(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "tcp_stream", "port_tunnel_limit_exceeded");
    refused_peer.reset();

    send_tunnel_frame(
        client_socket.get(),
        empty_frame(PortTunnelFrameType::Close, first_accept.stream_id)
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::Close);
    first_peer.reset();

    UniqueSocket second_peer(connect_port_forward_socket(endpoint, "tcp"));
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpAccept);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_queued_byte_limit_is_enforced(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_tunnel_queued_bytes = 128UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket payload_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(payload_listener.get());
    std::vector<unsigned char> payload(512U, 42U);
    std::thread sender_thread = accept_and_send_tcp_payload(payload_listener.get(), payload);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);

    assert_tunnel_error_code(
        read_tunnel_frame(client_socket.get()),
        "port_tunnel_limit_exceeded"
    );

    close_tunnel(&client_socket, &server_thread);
    payload_listener.reset();
    sender_thread.join();
}

static void assert_udp_queued_byte_pressure_reports_drop(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_tunnel_queued_bytes = 128UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    assert(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const std::string endpoint = Json::parse(bind_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer(bind_port_forward_socket("127.0.0.1:0", "udp"));
    socklen_t peer_len = 0;
    const sockaddr_storage destination = parse_port_forward_peer(endpoint, &peer_len);
    std::vector<unsigned char> payload(512U, 7U);
    assert(sendto(
        peer.get(),
        reinterpret_cast<const char*>(payload.data()),
        static_cast<int>(payload.size()),
        0,
        reinterpret_cast<const sockaddr*>(&destination),
        peer_len
    ) == static_cast<int>(payload.size()));

    PortTunnelFrame drop_report;
    assert(try_read_tunnel_frame_with_timeout(client_socket.get(), 1000UL, &drop_report));
    assert_forward_drop(drop_report, "udp_datagram", "port_tunnel_limit_exceeded");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_partial_tunnel_frame_times_out(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.tunnel_io_timeout_ms = 50UL;

    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);

    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    UniqueSocket client_socket(sockets[1]);
    std::thread server_thread = start_server_thread(state, &server_socket);

    send_all(
        client_socket.get(),
        "POST /v1/port/tunnel HTTP/1.1\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: remote-exec-port-tunnel\r\n"
        "X-Remote-Exec-Port-Tunnel-Version: 4\r\n"
        "\r\n"
    );
    const std::string response = read_http_head_from_socket(client_socket.get());
    assert(response.find("HTTP/1.1 101 Switching Protocols\r\n") == 0);
    send_preface(client_socket.get());

    const unsigned char partial_header[2] = {
        static_cast<unsigned char>(PortTunnelFrameType::TcpData),
        0U,
    };
    send_all_bytes(
        client_socket.get(),
        reinterpret_cast<const char*>(partial_header),
        sizeof(partial_header)
    );

    server_thread.join();
}

static void assert_tunnel_tcp_connect_echoes_binary_data(AppState& state) {
    UniqueSocket echo_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string echo_endpoint = socket_local_endpoint(echo_listener.get());
    std::thread echo_thread([&]() {
        UniqueSocket accepted(accept(echo_listener.get(), NULL, NULL));
        assert(accepted.valid());
        char buffer[64];
        const int received = recv(accepted.get(), buffer, sizeof(buffer), 0);
        assert(received > 0);
        send_all_bytes(accepted.get(), buffer, static_cast<std::size_t>(received));
    });

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", echo_endpoint}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);
    const std::vector<unsigned char> payload = {
        0U, 1U, 2U, 255U, static_cast<unsigned char>('x'), static_cast<unsigned char>('\n')
    };
    send_tunnel_frame(client_socket.get(), data_frame(PortTunnelFrameType::TcpData, 1U, payload));
    const PortTunnelFrame echoed = read_tunnel_frame(client_socket.get());
    assert(echoed.type == PortTunnelFrameType::TcpData);
    assert(echoed.data == payload);

    close_tunnel(&client_socket, &server_thread);
    echo_listener.reset();
    echo_thread.join();
}

static void assert_tcp_data_write_pressure_does_not_block_control_frames(AppState& state) {
    UniqueSocket hold_listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string hold_endpoint = socket_local_endpoint(hold_listener.get());
    std::thread hold_thread([&]() {
        UniqueSocket accepted(accept(hold_listener.get(), NULL, NULL));
        assert(accepted.valid());
        int buffer_size = 1024;
        setsockopt(
            accepted.get(),
            SOL_SOCKET,
            SO_RCVBUF,
            reinterpret_cast<const char*>(&buffer_size),
            sizeof(buffer_size)
        );
        platform::sleep_ms(5000UL);
    });

    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "connect", "tcp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", hold_endpoint}})
    );
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TcpConnectOk);

    std::vector<unsigned char> payload(PORT_TUNNEL_MAX_DATA_LEN, 0x51U);
    PortTunnelFrame heartbeat = empty_frame(PortTunnelFrameType::TunnelHeartbeat, 0U);
    heartbeat.meta = Json{{"nonce", 1}}.dump();
    std::thread writer_thread([&]() {
        for (int i = 0; i < 64; ++i) {
            send_tunnel_frame(
                client_socket.get(),
                data_frame(PortTunnelFrameType::TcpData, 1U, payload)
            );
        }
        send_tunnel_frame(client_socket.get(), heartbeat);
    });

    const std::uint64_t deadline = platform::monotonic_ms() + 2000ULL;
    bool saw_ack = false;
    while (platform::monotonic_ms() < deadline) {
        PortTunnelFrame frame;
        if (!try_read_tunnel_frame_with_timeout(client_socket.get(), 100UL, &frame)) {
            continue;
        }
        if (frame.type == PortTunnelFrameType::TunnelHeartbeatAck) {
            assert(frame.meta == heartbeat.meta);
            saw_ack = true;
            break;
        }
    }
    assert(saw_ack);

    close_tunnel(&client_socket, &server_thread);
    hold_listener.reset();
    hold_thread.join();
    writer_thread.join();
}

static void assert_tunnel_udp_bind_emits_two_peer_datagrams(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_v4_tunnel(state, &client_socket, &server_thread, "listen", "udp", 1ULL);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::UdpBind, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame bind_ok = read_tunnel_frame(client_socket.get());
    assert(bind_ok.type == PortTunnelFrameType::UdpBindOk);
    const std::string endpoint = Json::parse(bind_ok.meta).at("endpoint").get<std::string>();

    UniqueSocket peer_a(bind_port_forward_socket("127.0.0.1:0", "udp"));
    UniqueSocket peer_b(bind_port_forward_socket("127.0.0.1:0", "udp"));
    socklen_t peer_len = 0;
    const sockaddr_storage peer = parse_port_forward_peer(endpoint, &peer_len);
    assert(sendto(peer_a.get(), "udp-a", 5, 0, reinterpret_cast<const sockaddr*>(&peer), peer_len) == 5);
    assert(sendto(peer_b.get(), "udp-b", 5, 0, reinterpret_cast<const sockaddr*>(&peer), peer_len) == 5);

    const PortTunnelFrame first = read_tunnel_frame(client_socket.get());
    const PortTunnelFrame second = read_tunnel_frame(client_socket.get());
    assert(first.type == PortTunnelFrameType::UdpDatagram);
    assert(second.type == PortTunnelFrameType::UdpDatagram);
    std::vector<std::string> payloads;
    payloads.push_back(std::string(first.data.begin(), first.data.end()));
    payloads.push_back(std::string(second.data.begin(), second.data.end()));
    std::sort(payloads.begin(), payloads.end());
    assert(payloads[0] == "udp-a");
    assert(payloads[1] == "udp-b");

    close_tunnel(&client_socket, &server_thread);
}

static void assert_tunnel_tcp_listener_session_can_resume_after_transport_drop(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);

    open_v4_tunnel(
        state,
        &client_socket,
        &server_thread,
        "listen",
        "tcp",
        1ULL,
        session_id
    );

    UniqueSocket peer(connect_port_forward_socket(endpoint, "tcp"));
    assert(peer.valid());
    const PortTunnelFrame accepted = read_tunnel_frame(client_socket.get());
    assert(accepted.type == PortTunnelFrameType::TcpAccept);

    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelClose,
            0U,
            Json{
                {"forward_id", "fwd_cpp_test"},
                {"generation", 1ULL},
                {"reason", "operator_close"}
            }
        )
    );
    const PortTunnelFrame closed = read_tunnel_frame(client_socket.get());
    assert(closed.type == PortTunnelFrameType::TunnelClosed);
    assert(Json::parse(closed.meta).at("generation").get<uint64_t>() == 1ULL);

    close_tunnel(&client_socket, &server_thread);
}

static void assert_expired_tunnel_session_is_released(AppState& state) {
    UniqueSocket client_socket;
    std::thread server_thread;
    const PortTunnelFrame ready =
        open_v4_tunnel(state, &client_socket, &server_thread, "listen", "tcp", 1ULL);
    const Json ready_meta = Json::parse(ready.meta);
    const std::string session_id = ready_meta.at("session_id").get<std::string>();
    const unsigned long resume_timeout_ms =
        ready_meta.at("resume_timeout_ms").get<unsigned long>();

    send_tunnel_frame(
        client_socket.get(),
        json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}})
    );
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();

    close_tunnel(&client_socket, &server_thread);
    platform::sleep_ms(resume_timeout_ms + 200UL);

    open_tunnel(state, &client_socket, &server_thread);
    send_tunnel_frame(
        client_socket.get(),
        json_frame(
            PortTunnelFrameType::TunnelOpen,
            0U,
            tunnel_open_meta("listen", "tcp", 1ULL, session_id)
        )
    );
    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    assert(error.type == PortTunnelFrameType::Error);
    const Json error_meta = Json::parse(error.meta);
    assert(error_meta.at("code").get<std::string>() == "port_tunnel_resume_expired");

    close_tunnel(&client_socket, &server_thread);

    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    assert(rebound.valid());
}

int main() {
    NetworkSession network;
    const fs::path root = make_test_root();
    AppState state;
    initialize_state(state, root);

    assert_persistent_json_requests_reuse_socket(state);

    const std::string archive = tar_with_single_file("streamed import");
    const fs::path imported_path = root / "imported.txt";
    std::ostringstream import_request;
    import_request << "POST /v1/transfer/import HTTP/1.1\r\n"
                   << "Transfer-Encoding: chunked\r\n"
                   << "x-remote-exec-source-type: file\r\n"
                   << "x-remote-exec-destination-path: " << imported_path.string() << "\r\n"
                   << "x-remote-exec-overwrite: replace\r\n"
                   << "x-remote-exec-create-parent: true\r\n"
                   << "x-remote-exec-symlink-mode: preserve\r\n"
                   << "x-remote-exec-compression: none\r\n"
                   << "\r\n"
                   << chunked_body(archive);

    const std::string import_response = run_single_request(state, import_request.str());
    assert(import_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(read_text_file(imported_path) == "streamed import");

    const fs::path export_path = root / "export.txt";
    write_text_file(export_path, "streamed export");
    const std::string export_body = Json{{"path", export_path.string()}}.dump();
    std::ostringstream export_request;
    export_request << "POST /v1/transfer/export HTTP/1.1\r\n"
                   << "Content-Length: " << export_body.size() << "\r\n"
                   << "\r\n"
                   << export_body;

    const std::string export_response = run_single_request(state, export_request.str());
    assert(export_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(export_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos);
    assert(export_response.find("Connection: close\r\n") == std::string::npos);
    assert(export_response.find("Content-Length:") == std::string::npos);
    assert(export_response.find("x-remote-exec-source-type: file\r\n") != std::string::npos);
    assert(single_file_tar_body(decode_chunked_response_body(export_response)) == "streamed export");

    const fs::path sandbox_root = root / "sandbox";
    const fs::path read_allowed = sandbox_root / "read";
    const fs::path write_allowed = sandbox_root / "write";
    const fs::path outside = sandbox_root / "outside";
    fs::create_directories(read_allowed);
    fs::create_directories(write_allowed);
    fs::create_directories(outside);
    write_text_file(outside / "outside.txt", "outside");

    AppState sandbox_state;
    initialize_state(sandbox_state, root);
    sandbox_state.config.sandbox_configured = true;
    sandbox_state.config.sandbox.read.allow.push_back(read_allowed.string());
    sandbox_state.config.sandbox.write.allow.push_back(write_allowed.string());
    enable_sandbox(sandbox_state);

    const std::string denied_export_body = Json{{"path", (outside / "outside.txt").string()}}.dump();
    std::ostringstream denied_export_request;
    denied_export_request << "POST /v1/transfer/export HTTP/1.1\r\n"
                          << "Content-Length: " << denied_export_body.size() << "\r\n"
                          << "\r\n"
                          << denied_export_body;
    const std::string denied_export_response =
        run_single_request(sandbox_state, denied_export_request.str());
    assert(denied_export_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    assert(Json::parse(response_body(denied_export_response)).at("code").get<std::string>() == "sandbox_denied");

    std::ostringstream denied_import_request;
    denied_import_request << "POST /v1/transfer/import HTTP/1.1\r\n"
                          << "Transfer-Encoding: chunked\r\n"
                          << "x-remote-exec-source-type: file\r\n"
                          << "x-remote-exec-destination-path: " << (outside / "imported.txt").string() << "\r\n"
                          << "x-remote-exec-overwrite: replace\r\n"
                          << "x-remote-exec-create-parent: true\r\n"
                          << "x-remote-exec-symlink-mode: preserve\r\n"
                          << "x-remote-exec-compression: none\r\n"
                          << "\r\n"
                          << chunked_body(archive);
    const std::string denied_import_response =
        run_single_request(sandbox_state, denied_import_request.str());
    assert(denied_import_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    assert(Json::parse(response_body(denied_import_response)).at("code").get<std::string>() == "sandbox_denied");
    assert(!fs::exists(outside / "imported.txt"));

    assert_tunnel_close_releases_tcp_listener(state);
    assert_terminal_tunnel_error_releases_tcp_listener_immediately(state);
    assert_tunnel_open_ready_and_close_round_trip(state);
    assert_tunnel_close_releases_retained_listener_immediately(state);
    assert_port_tunnel_worker_limit_is_reported(root);
    assert_tcp_connect_worker_limit_errors_before_success(root);
    assert_tcp_connect_read_thread_failure_errors_before_success(root);
    assert_tcp_accept_read_thread_failure_drops_before_accept(root);
    assert_retained_tcp_accept_read_thread_failure_drops_before_accept(root);
    assert_retained_tcp_accept_worker_pressure_is_local_drop(root);
    assert_retained_tcp_accept_pressure_is_local_drop(root);
    assert_tunnel_ready_reports_configured_limits(root);
    assert_tunnel_rejects_data_plane_before_open(state);
    assert_tunnel_rejects_frames_for_wrong_role_or_protocol(state);
    assert_legacy_session_frames_are_reserved_but_unsupported(state);
    assert_retained_session_limit_is_enforced(root);
    assert_retained_listener_limit_is_enforced_and_released(root);
    assert_udp_bind_limit_is_enforced_and_released(root);
    assert_active_tcp_stream_limit_is_enforced_and_released(root);
    assert_active_tcp_accept_limit_is_enforced_and_released(root);
    assert_tunnel_queued_byte_limit_is_enforced(root);
    assert_udp_queued_byte_pressure_reports_drop(root);
    assert_partial_tunnel_frame_times_out(root);
    assert_tunnel_tcp_connect_echoes_binary_data(state);
    assert_tcp_data_write_pressure_does_not_block_control_frames(state);
    assert_tunnel_udp_bind_emits_two_peer_datagrams(state);
    assert_tunnel_tcp_listener_session_can_resume_after_transport_drop(state);
    assert_expired_tunnel_session_is_released(state);

    return 0;
}
