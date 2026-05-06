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
#include <sys/socket.h>

#include "config.h"
#include "filesystem_sandbox.h"
#include "http_helpers.h"
#include "path_policy.h"
#include "platform.h"
#include "port_forward_endpoint.h"
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
    config.yield_time = default_yield_time_config();
    return config;
}

static void initialize_state(AppState& state, const fs::path& root) {
    state.config = make_config(root);
    state.daemon_instance_id = "test-instance";
    state.hostname = "test-host";
    state.default_shell = platform::resolve_default_shell("");
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

static void run_single_request_and_abort_client(AppState& state, const std::string& request) {
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    UniqueSocket client_socket(sockets[1]);
    send_request_and_close_writer(client_socket.get(), request);
    client_socket.reset();
    handle_client(state, std::move(server_socket));
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

    const Json tcp_listen = Json::parse(
        response_body(
            run_single_request(
                state,
                json_post_request(
                    "/v1/port/listen",
                    Json{{"endpoint", "127.0.0.1:0"}, {"protocol", "tcp"}}
                )
            )
        )
    );
    const std::string accept_bind_id = tcp_listen.at("bind_id").get<std::string>();
    std::string accept_response;
    std::thread accept_thread([&]() {
        accept_response = run_single_request(
            state,
            json_post_request("/v1/port/listen/accept", Json{{"bind_id", accept_bind_id}})
        );
    });
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    const std::string close_accept_response = run_single_request(
        state,
        json_post_request("/v1/port/listen/close", Json{{"bind_id", accept_bind_id}})
    );
    assert(close_accept_response.find("HTTP/1.1 200 OK\r\n") == 0);
    accept_thread.join();
    assert(accept_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    assert(Json::parse(response_body(accept_response)).at("code").get<std::string>() == "port_bind_closed");

    const Json udp_listen = Json::parse(
        response_body(
            run_single_request(
                state,
                json_post_request(
                    "/v1/port/listen",
                    Json{{"endpoint", "127.0.0.1:0"}, {"protocol", "udp"}}
                )
            )
        )
    );
    const std::string udp_bind_id = udp_listen.at("bind_id").get<std::string>();
    std::string udp_read_response;
    std::thread udp_read_thread([&]() {
        udp_read_response = run_single_request(
            state,
            json_post_request("/v1/port/udp/read", Json{{"bind_id", udp_bind_id}})
        );
    });
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    const std::string close_udp_response = run_single_request(
        state,
        json_post_request("/v1/port/listen/close", Json{{"bind_id", udp_bind_id}})
    );
    assert(close_udp_response.find("HTTP/1.1 200 OK\r\n") == 0);
    udp_read_thread.join();
    assert(udp_read_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    assert(Json::parse(response_body(udp_read_response)).at("code").get<std::string>() == "port_bind_closed");

    const Json read_listen = Json::parse(
        response_body(
            run_single_request(
                state,
                json_post_request(
                    "/v1/port/listen",
                    Json{{"endpoint", "127.0.0.1:0"}, {"protocol", "tcp"}}
                )
            )
        )
    );
    int accepted_socket = socket(AF_INET, SOCK_STREAM, 0);
    assert(accepted_socket != INVALID_SOCKET);
    const ParsedPortForwardEndpoint accepted_endpoint =
        parse_port_forward_endpoint(read_listen.at("endpoint").get<std::string>());
    addrinfo hints;
    std::memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo* resolved = NULL;
    assert(
        getaddrinfo(
            accepted_endpoint.host.c_str(),
            accepted_endpoint.port.c_str(),
            &hints,
            &resolved
        ) == 0
    );
    assert(connect(accepted_socket, resolved->ai_addr, static_cast<int>(resolved->ai_addrlen)) == 0);
    freeaddrinfo(resolved);
    const Json accepted = Json::parse(
        response_body(
            run_single_request(
                state,
                json_post_request(
                    "/v1/port/listen/accept",
                    Json{{"bind_id", read_listen.at("bind_id").get<std::string>()}}
                )
            )
        )
    );
    const std::string read_connection_id = accepted.at("connection_id").get<std::string>();
    std::string read_response;
    std::thread read_thread([&]() {
        read_response = run_single_request(
            state,
            json_post_request(
                "/v1/port/connection/read",
                Json{{"connection_id", read_connection_id}}
            )
        );
    });
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    const std::string close_connection_response = run_single_request(
        state,
        json_post_request(
            "/v1/port/connection/close",
            Json{{"connection_id", read_connection_id}}
        )
    );
    assert(close_connection_response.find("HTTP/1.1 200 OK\r\n") == 0);
    read_thread.join();
    close_socket(accepted_socket);
    assert(read_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    assert(
        Json::parse(response_body(read_response)).at("code").get<std::string>() ==
        "port_connection_closed"
    );

    const Json leased_listen = Json::parse(
        response_body(
            run_single_request(
                state,
                json_post_request(
                    "/v1/port/listen",
                    Json{
                        {"endpoint", "127.0.0.1:0"},
                        {"protocol", "tcp"},
                        {"lease", Json{{"lease_id", "lease-late-renew"}, {"ttl_ms", 300}}}
                    }
                )
            )
        )
    );
    const std::string leased_endpoint = leased_listen.at("endpoint").get<std::string>();
    std::this_thread::sleep_for(std::chrono::milliseconds(500));
    const std::string renew_response = run_single_request(
        state,
        json_post_request(
            "/v1/port/lease/renew",
            Json{{"lease_id", "lease-late-renew"}, {"ttl_ms", 300}}
        )
    );
    assert(renew_response.find("HTTP/1.1 200 OK\r\n") == 0);
    const Json rebound_listen = Json::parse(
        response_body(
            run_single_request(
                state,
                json_post_request(
                    "/v1/port/listen",
                    Json{{"endpoint", leased_endpoint}, {"protocol", "tcp"}}
                )
            )
        )
    );
    assert(rebound_listen.at("endpoint").get<std::string>() == leased_endpoint);
    const std::string close_rebound_response = run_single_request(
        state,
        json_post_request(
            "/v1/port/listen/close",
            Json{{"bind_id", rebound_listen.at("bind_id").get<std::string>()}}
        )
    );
    assert(close_rebound_response.find("HTTP/1.1 200 OK\r\n") == 0);

    run_single_request_and_abort_client(
        state,
        json_post_request(
            "/v1/port/connection/read",
            Json{{"connection_id", "missing-connection"}}
        )
    );

    return 0;
}
