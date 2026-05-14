#include "test_server_streaming_shared.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>

#include <sys/socket.h>

#include "base64_codec.h"
#include "path_policy.h"

static std::size_t response_content_length(const std::string& header_block) {
    const std::string marker = "\r\nContent-Length: ";
    const std::size_t start = header_block.find(marker);
    TEST_ASSERT(start != std::string::npos);
    const std::size_t value_start = start + marker.size();
    const std::size_t value_end = header_block.find("\r\n", value_start);
    TEST_ASSERT(value_end != std::string::npos);
    return static_cast<std::size_t>(
        std::strtoull(header_block.substr(value_start, value_end - value_start).c_str(), NULL, 10));
}

static std::string read_content_length_response_from_socket(SOCKET socket) {
    std::string response;
    while (response.find("\r\n\r\n") == std::string::npos) {
        char ch = '\0';
        const int received = recv(socket, &ch, 1, 0);
        TEST_ASSERT(received > 0);
        response.push_back(ch);
    }

    const std::size_t header_end = response.find("\r\n\r\n");
    const std::size_t content_length = response_content_length(response.substr(0, header_end));
    const std::size_t total_size = header_end + 4U + content_length;
    while (response.size() < total_size) {
        char buffer[4096];
        const std::size_t remaining = total_size - response.size();
        const std::size_t request_size = std::min<std::size_t>(remaining, sizeof(buffer));
        const int received = recv(socket, buffer, static_cast<int>(request_size), 0);
        TEST_ASSERT(received > 0);
        response.append(buffer, static_cast<std::size_t>(received));
    }

    return response;
}

static std::string read_text_file(const fs::path& path) {
    return fs::read_file_bytes(path);
}

static void write_text_file(const fs::path& path, const std::string& value) {
    fs::write_file_bytes(path, value);
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

static void send_request_and_close_writer(SOCKET socket, const std::string& request) {
    send_all(socket, request);
    shutdown(socket, SHUT_WR);
}

static std::string response_body(const std::string& response) {
    const std::size_t header_end = response.find("\r\n\r\n");
    TEST_ASSERT(header_end != std::string::npos);
    return response.substr(header_end + 4);
}

static std::string decode_chunked_response_body(const std::string& response) {
    const std::string body = response_body(response);
    std::string decoded;
    std::size_t offset = 0;
    for (;;) {
        const std::size_t line_end = body.find("\r\n", offset);
        TEST_ASSERT(line_end != std::string::npos);
        std::size_t chunk_size = 0;
        std::istringstream size_stream(body.substr(offset, line_end - offset));
        size_stream >> std::hex >> chunk_size;
        offset = line_end + 2;
        if (chunk_size == 0U) {
            TEST_ASSERT(body.compare(offset, 2, "\r\n") == 0);
            return decoded;
        }
        TEST_ASSERT(offset + chunk_size + 2 <= body.size());
        decoded.append(body, offset, chunk_size);
        offset += chunk_size;
        TEST_ASSERT(body.compare(offset, 2, "\r\n") == 0);
        offset += 2;
    }
}

static std::string octal_field(std::size_t width, std::uint64_t value) {
    char buffer[64];
    std::snprintf(
        buffer, sizeof(buffer), "%0*llo", static_cast<int>(width - 1), static_cast<unsigned long long>(value));
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
    TEST_ASSERT(archive.size() >= 512);
    const char* header = archive.data();
    std::size_t path_length = 0;
    while (path_length < 100 && header[path_length] != '\0') {
        ++path_length;
    }
    TEST_ASSERT(std::string(header, path_length) == ".remote-exec-file");
    TEST_ASSERT(header[156] == '0');
    const std::uint64_t size = parse_octal_value(header + 124, 12);
    TEST_ASSERT(512 + static_cast<std::size_t>(size) <= archive.size());
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

static std::string encoded_destination_path_header(const fs::path& destination) {
    return base64_encode_bytes(destination.string());
}

static std::string run_single_request(AppState& state, const std::string& request) {
    int sockets[2];
    TEST_ASSERT(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

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

static std::string
json_post_request_with_extra_headers(const std::string& path, const Json& body, const std::string& extra_headers) {
    const std::string payload = body.dump();
    std::ostringstream request;
    request << "POST " << path << " HTTP/1.1\r\n"
            << "Content-Length: " << payload.size() << "\r\n"
            << extra_headers << "\r\n"
            << payload;
    return request.str();
}

static void assert_persistent_json_requests_reuse_socket(AppState& state) {
    int sockets[2];
    TEST_ASSERT(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    UniqueSocket client_socket(sockets[1]);
    std::thread server_thread(
        [&state](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(state, std::move(owned_socket));
        },
        server_socket.release());

    send_all(client_socket.get(), json_post_request("/v1/health", Json::object()));
    const std::string first_response = read_content_length_response_from_socket(client_socket.get());
    TEST_ASSERT(first_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(first_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(Json::parse(response_body(first_response)).at("status").get<std::string>() == "ok");

    send_all(client_socket.get(),
             json_post_request_with_extra_headers("/v1/target-info", Json::object(), "Connection: close\r\n"));
    const std::string second_response = read_content_length_response_from_socket(client_socket.get());
    TEST_ASSERT(second_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(second_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(Json::parse(response_body(second_response)).at("target").get<std::string>() == "cpp-test");

    char extra = '\0';
    TEST_ASSERT(recv(client_socket.get(), &extra, 1, 0) == 0);
    server_thread.join();
}

void assert_http_streaming_routes(AppState& state, const fs::path& root) {
    assert_persistent_json_requests_reuse_socket(state);

    const std::string archive = tar_with_single_file("streamed import");
    const fs::path imported_path = root / "imported.txt";
    std::ostringstream import_request;
    import_request << "POST /v1/transfer/import HTTP/1.1\r\n"
                   << "Transfer-Encoding: chunked\r\n"
                   << "x-remote-exec-source-type: file\r\n"
                   << "x-remote-exec-destination-path: " << encoded_destination_path_header(imported_path) << "\r\n"
                   << "x-remote-exec-overwrite: replace\r\n"
                   << "x-remote-exec-create-parent: true\r\n"
                   << "x-remote-exec-symlink-mode: preserve\r\n"
                   << "x-remote-exec-compression: none\r\n"
                   << "\r\n"
                   << chunked_body(archive);

    const std::string import_response = run_single_request(state, import_request.str());
    TEST_ASSERT(import_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(read_text_file(imported_path) == "streamed import");

    const fs::path export_path = root / "export.txt";
    write_text_file(export_path, "streamed export");
    const std::string export_body = Json{{"path", export_path.string()}}.dump();
    std::ostringstream export_request;
    export_request << "POST /v1/transfer/export HTTP/1.1\r\n"
                   << "Content-Length: " << export_body.size() << "\r\n"
                   << "\r\n"
                   << export_body;

    const std::string export_response = run_single_request(state, export_request.str());
    TEST_ASSERT(export_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(export_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos);
    TEST_ASSERT(export_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(export_response.find("Content-Length:") == std::string::npos);
    TEST_ASSERT(export_response.find("x-remote-exec-source-type: file\r\n") != std::string::npos);
    TEST_ASSERT(single_file_tar_body(decode_chunked_response_body(export_response)) == "streamed export");

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
    const std::string denied_export_response = run_single_request(sandbox_state, denied_export_request.str());
    TEST_ASSERT(denied_export_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    TEST_ASSERT(Json::parse(response_body(denied_export_response)).at("code").get<std::string>() == "sandbox_denied");

    std::ostringstream denied_import_request;
    denied_import_request << "POST /v1/transfer/import HTTP/1.1\r\n"
                          << "Transfer-Encoding: chunked\r\n"
                          << "x-remote-exec-source-type: file\r\n"
                          << "x-remote-exec-destination-path: "
                          << encoded_destination_path_header(outside / "imported.txt") << "\r\n"
                          << "x-remote-exec-overwrite: replace\r\n"
                          << "x-remote-exec-create-parent: true\r\n"
                          << "x-remote-exec-symlink-mode: preserve\r\n"
                          << "x-remote-exec-compression: none\r\n"
                          << "\r\n"
                          << chunked_body(archive);
    const std::string denied_import_response = run_single_request(sandbox_state, denied_import_request.str());
    TEST_ASSERT(denied_import_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    TEST_ASSERT(Json::parse(response_body(denied_import_response)).at("code").get<std::string>() == "sandbox_denied");
    TEST_ASSERT(!fs::exists(outside / "imported.txt"));
}
