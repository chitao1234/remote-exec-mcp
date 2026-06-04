#include "test_server_streaming_shared.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>

#include "codec/base64_codec.h"
#include "policy/path_policy.h"
#include "test_socket_pair.h"

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
#ifdef _WIN32
    shutdown(socket, SD_SEND);
#else
    shutdown(socket, SHUT_WR);
#endif
}

static std::string response_body(const std::string& response) {
    const std::size_t header_end = response.find("\r\n\r\n");
    TEST_ASSERT(header_end != std::string::npos);
    return response.substr(header_end + 4);
}

static void assert_json_response_code(const std::string& response,
                                      const std::string& status_line,
                                      const std::string& code) {
    TEST_ASSERT(response.find(status_line) == 0);
    TEST_ASSERT(Json::parse(response_body(response)).at("code").get<std::string>() == code);
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

static std::string chunked_data_only(const std::string& body) {
    std::ostringstream out;
    out << std::hex << body.size() << "\r\n";
    out.write(body.data(), static_cast<std::streamsize>(body.size()));
    out << "\r\n";
    return out.str();
}

static void append_u64_be(std::string* output, std::uint64_t value) {
    for (std::size_t i = 0; i < 8U; ++i) {
        output->push_back(static_cast<char>((value >> ((7U - i) * 8U)) & 0xffU));
    }
}

static std::uint64_t read_u64_be(const std::string& value, std::size_t offset) {
    std::uint64_t output = 0U;
    for (std::size_t i = 0; i < 8U; ++i) {
        output = (output << 8U) | static_cast<unsigned char>(value[offset + i]);
    }
    return output;
}

static std::string transfer_frame(unsigned char frame_type, const std::string& payload) {
    std::string output;
    output.push_back(static_cast<char>(frame_type));
    output.append(3, '\0');
    append_u64_be(&output, static_cast<std::uint64_t>(payload.size()));
    output.append(payload);
    return output;
}

static std::string framed_transfer_body(const std::string& archive) {
    std::string output = "REXFER2\n";
    output += transfer_frame(0x01, archive);
    output += transfer_frame(0x02, Json{{"archive_bytes", archive.size()}}.dump());
    return output;
}

static std::string decode_framed_transfer_archive(const std::string& body) {
    TEST_ASSERT(body.compare(0, 8, "REXFER2\n") == 0);
    std::size_t offset = 8U;
    std::string archive;
    for (;;) {
        TEST_ASSERT(offset + 12U <= body.size());
        const unsigned char frame_type = static_cast<unsigned char>(body[offset]);
        TEST_ASSERT(body[offset + 1U] == '\0');
        TEST_ASSERT(body[offset + 2U] == '\0');
        TEST_ASSERT(body[offset + 3U] == '\0');
        const std::uint64_t payload_len = read_u64_be(body, offset + 4U);
        offset += 12U;
        TEST_ASSERT(payload_len <= static_cast<std::uint64_t>(body.size() - offset));
        const std::string payload = body.substr(offset, static_cast<std::size_t>(payload_len));
        offset += static_cast<std::size_t>(payload_len);
        if (frame_type == 0x01U) {
            archive += payload;
            continue;
        }
        if (frame_type == 0x02U) {
            TEST_ASSERT(Json::parse(payload).at("archive_bytes").get<std::uint64_t>() == archive.size());
            return archive;
        }
        TEST_ASSERT(false);
    }
}

static Json decode_framed_transfer_error(const std::string& body) {
    TEST_ASSERT(body.compare(0, 8, "REXFER2\n") == 0);
    std::size_t offset = 8U;
    for (;;) {
        TEST_ASSERT(offset + 12U <= body.size());
        const unsigned char frame_type = static_cast<unsigned char>(body[offset]);
        const std::uint64_t payload_len = read_u64_be(body, offset + 4U);
        offset += 12U;
        TEST_ASSERT(payload_len <= static_cast<std::uint64_t>(body.size() - offset));
        const std::string payload = body.substr(offset, static_cast<std::size_t>(payload_len));
        offset += static_cast<std::size_t>(payload_len);
        if (frame_type == 0x03U) {
            return Json::parse(payload);
        }
    }
}

static std::string encoded_destination_path_header(const fs::path& destination) {
    return base64_encode_bytes(destination.string());
}

static std::string run_single_request(AppState& state, const std::string& request) {
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
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
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
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

static void assert_http_auth_and_rejection_paths(AppState& state, const fs::path& root) {
    const std::string missing_auth_response = run_single_request(state, json_post_request("/v1/health", Json::object()));
    assert_json_response_code(missing_auth_response, "HTTP/1.1 401 Unauthorized\r\n", "unauthorized");
    TEST_ASSERT(missing_auth_response.find("WWW-Authenticate: Bearer\r\n") != std::string::npos);

    const std::string wrong_auth_response =
        run_single_request(state,
                           json_post_request_with_extra_headers(
                               "/v1/health", Json::object(), "Authorization: Bearer wrong-secret\r\n"));
    assert_json_response_code(wrong_auth_response, "HTTP/1.1 401 Unauthorized\r\n", "unauthorized");

    const std::string ok_response =
        run_single_request(state,
                           json_post_request_with_extra_headers(
                               "/v1/health", Json::object(), "Authorization: Bearer shared-secret\r\n"));
    TEST_ASSERT(ok_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(Json::parse(response_body(ok_response)).at("status").get<std::string>() == "ok");

    AppState unauthenticated_state;
    initialize_state(unauthenticated_state, root);

    const std::string get_response = run_single_request(
        unauthenticated_state,
        "GET /v1/health HTTP/1.1\r\n"
        "Content-Length: 0\r\n"
        "\r\n");
    assert_json_response_code(get_response, "HTTP/1.1 405 Method Not Allowed\r\n", "method_not_allowed");

    const std::string not_found_response =
        run_single_request(unauthenticated_state, json_post_request("/v1/not-found", Json::object()));
    assert_json_response_code(not_found_response, "HTTP/1.1 404 Not Found\r\n", "not_found");
}

static void assert_http_request_limits_through_connection_path(const fs::path& root) {
    AppState header_limit_state;
    initialize_state(header_limit_state, root);
    header_limit_state.config.max_request_header_bytes = 48U;

    std::ostringstream oversized_header_request;
    oversized_header_request << "POST /v1/health HTTP/1.1\r\n"
                             << "X-Too-Large: " << std::string(80, 'x') << "\r\n"
                             << "\r\n";
    const std::string header_limit_response =
        run_single_request(header_limit_state, oversized_header_request.str());
    assert_json_response_code(header_limit_response, "HTTP/1.1 400 Bad Request\r\n", "bad_request");
    TEST_ASSERT(response_body(header_limit_response).find("http request headers too large") != std::string::npos);

    AppState content_length_limit_state;
    initialize_state(content_length_limit_state, root);
    content_length_limit_state.config.max_request_body_bytes = 4U;

    const std::string content_length_limit_response = run_single_request(
        content_length_limit_state,
        "POST /v1/health HTTP/1.1\r\n"
        "Content-Length: 5\r\n"
        "\r\n"
        "12345");
    assert_json_response_code(content_length_limit_response, "HTTP/1.1 400 Bad Request\r\n", "bad_request");
    TEST_ASSERT(response_body(content_length_limit_response).find("http request body too large") != std::string::npos);

    AppState chunked_limit_state;
    initialize_state(chunked_limit_state, root);
    chunked_limit_state.config.max_request_body_bytes = 4U;

    const std::string chunked_limit_response = run_single_request(
        chunked_limit_state,
        "POST /v1/health HTTP/1.1\r\n"
        "Transfer-Encoding: chunked\r\n"
        "\r\n"
        "5\r\n"
        "12345\r\n"
        "0\r\n"
        "\r\n");
    assert_json_response_code(chunked_limit_response, "HTTP/1.1 400 Bad Request\r\n", "bad_request");
    TEST_ASSERT(response_body(chunked_limit_response).find("http request body too large") != std::string::npos);
}

static void assert_streaming_import_ignores_generic_http_body_limit(const fs::path& root) {
    AppState state;
    initialize_state(state, root);
    state.config.max_request_body_bytes = 32U;
    state.config.transfer_limits.max_archive_bytes = 4096U;
    state.config.transfer_limits.max_entry_bytes = 4096U;

    const std::string archive = tar_with_single_file("stream body exceeds generic http limit");
    TEST_ASSERT(framed_transfer_body(archive).size() > state.config.max_request_body_bytes);

    const fs::path imported_path = root / "small-http-limit-import.txt";
    std::ostringstream request;
    request << "POST /v1/transfer/import HTTP/1.1\r\n"
            << "Transfer-Encoding: chunked\r\n"
            << "Content-Type: application/vnd.remote-exec.transfer-stream.v2\r\n"
            << "x-remote-exec-transfer-stream-version: 2\r\n"
            << "x-remote-exec-source-type: file\r\n"
            << "x-remote-exec-destination-path: " << encoded_destination_path_header(imported_path) << "\r\n"
            << "x-remote-exec-overwrite: replace\r\n"
            << "x-remote-exec-create-parent: true\r\n"
            << "x-remote-exec-symlink-mode: preserve\r\n"
            << "x-remote-exec-compression: none\r\n"
            << "\r\n"
            << chunked_body(framed_transfer_body(archive));

    const std::string response = run_single_request(state, request.str());
    TEST_ASSERT(response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(read_text_file(imported_path) == "stream body exceeds generic http limit");
}

static void assert_streaming_import_failure_closes_connection_with_unread_body(const fs::path& root) {
    AppState state;
    initialize_state(state, root);
    state.config.transfer_limits.max_archive_bytes = 16U;
    state.config.transfer_limits.max_entry_bytes = 4096U;

    const std::string archive = tar_with_single_file(std::string(128U, 'x'));
    const std::string framed = framed_transfer_body(archive);
    const fs::path imported_path = root / "failed-import.txt";

    std::ostringstream request_head;
    request_head << "POST /v1/transfer/import HTTP/1.1\r\n"
                 << "Transfer-Encoding: chunked\r\n"
                 << "Content-Type: application/vnd.remote-exec.transfer-stream.v2\r\n"
                 << "x-remote-exec-transfer-stream-version: 2\r\n"
                 << "x-remote-exec-source-type: file\r\n"
                 << "x-remote-exec-destination-path: " << encoded_destination_path_header(imported_path) << "\r\n"
                 << "x-remote-exec-overwrite: replace\r\n"
                 << "x-remote-exec-create-parent: true\r\n"
                 << "x-remote-exec-symlink-mode: preserve\r\n"
                 << "x-remote-exec-compression: none\r\n"
                 << "\r\n";

    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
    std::thread server_thread(
        [&state](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(state, std::move(owned_socket));
        },
        server_socket.release());

    send_all(client_socket.get(), request_head.str());

    const std::size_t split_offset = 24U;
    send_all(client_socket.get(), chunked_data_only(framed.substr(0U, split_offset)));

    send_all(client_socket.get(), chunked_body(framed.substr(split_offset)));

    const std::string response = read_content_length_response_from_socket(client_socket.get());
    assert_json_response_code(response, "HTTP/1.1 400 Bad Request\r\n", "transfer_failed");

    assert_socket_closed_within(client_socket.get(), 5000UL);
    TEST_ASSERT(!fs::exists(imported_path));
    server_thread.join();
}

void assert_http_streaming_routes(AppState& state, const fs::path& root) {
    assert_persistent_json_requests_reuse_socket(state);

    AppState auth_state;
    initialize_state(auth_state, root);
    auth_state.config.http_auth_bearer_token = "shared-secret";
    assert_http_auth_and_rejection_paths(auth_state, root);
    assert_http_request_limits_through_connection_path(root);
    assert_streaming_import_ignores_generic_http_body_limit(root);
    assert_streaming_import_failure_closes_connection_with_unread_body(root);

    const std::string archive = tar_with_single_file("streamed import");
    const fs::path imported_path = root / "imported.txt";
    std::ostringstream import_request;
    import_request << "POST /v1/transfer/import HTTP/1.1\r\n"
                   << "Transfer-Encoding: chunked\r\n"
                   << "Content-Type: application/vnd.remote-exec.transfer-stream.v2\r\n"
                   << "x-remote-exec-transfer-stream-version: 2\r\n"
                   << "x-remote-exec-source-type: file\r\n"
                   << "x-remote-exec-destination-path: " << encoded_destination_path_header(imported_path) << "\r\n"
                   << "x-remote-exec-overwrite: replace\r\n"
                   << "x-remote-exec-create-parent: true\r\n"
                   << "x-remote-exec-symlink-mode: preserve\r\n"
                   << "x-remote-exec-compression: none\r\n"
                   << "\r\n"
                   << chunked_body(framed_transfer_body(archive));

    const std::string import_response = run_single_request(state, import_request.str());
    TEST_ASSERT(import_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(read_text_file(imported_path) == "streamed import");

    const fs::path export_path = root / "export.txt";
    write_text_file(export_path, "streamed export");
    const std::string export_body = Json{{"path", export_path.string()}}.dump();
    std::ostringstream export_request;
    export_request << "POST /v1/transfer/export HTTP/1.1\r\n"
                   << "Content-Length: " << export_body.size() << "\r\n"
                   << "x-remote-exec-transfer-stream-version: 2\r\n"
                   << "\r\n"
                   << export_body;

    const std::string export_response = run_single_request(state, export_request.str());
    TEST_ASSERT(export_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(export_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos);
    TEST_ASSERT(export_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(export_response.find("Content-Length:") == std::string::npos);
    TEST_ASSERT(export_response.find("x-remote-exec-source-type: file\r\n") != std::string::npos);
    TEST_ASSERT(single_file_tar_body(decode_framed_transfer_archive(decode_chunked_response_body(export_response))) ==
                "streamed export");

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
                          << "x-remote-exec-transfer-stream-version: 2\r\n"
                          << "\r\n"
                          << denied_export_body;
    const std::string denied_export_response = run_single_request(sandbox_state, denied_export_request.str());
    TEST_ASSERT(denied_export_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    TEST_ASSERT(Json::parse(response_body(denied_export_response)).at("code").get<std::string>() == "sandbox_denied");

    const fs::path recursive_read_root = read_allowed / "recursive";
    const fs::path recursive_denied = recursive_read_root / "secret";
    fs::create_directories(recursive_denied);
    write_text_file(recursive_read_root / "visible.txt", "visible");
    write_text_file(recursive_denied / "hidden.txt", "hidden");

    AppState recursive_deny_state;
    initialize_state(recursive_deny_state, root);
    recursive_deny_state.config.sandbox_configured = true;
    recursive_deny_state.config.sandbox.read.allow.push_back(read_allowed.string());
    recursive_deny_state.config.sandbox.read.deny.push_back(recursive_denied.string());
    enable_sandbox(recursive_deny_state);

    const std::string recursive_deny_body = Json{{"path", recursive_read_root.string()}}.dump();
    std::ostringstream recursive_deny_request;
    recursive_deny_request << "POST /v1/transfer/export HTTP/1.1\r\n"
                           << "Content-Length: " << recursive_deny_body.size() << "\r\n"
                           << "x-remote-exec-transfer-stream-version: 2\r\n"
                           << "\r\n"
                           << recursive_deny_body;
    const std::string recursive_deny_response = run_single_request(recursive_deny_state, recursive_deny_request.str());
    TEST_ASSERT(recursive_deny_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(recursive_deny_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos);
    TEST_ASSERT(decode_framed_transfer_error(decode_chunked_response_body(recursive_deny_response))
                    .at("code")
                    .get<std::string>() == "sandbox_denied");

    std::ostringstream denied_import_request;
    denied_import_request << "POST /v1/transfer/import HTTP/1.1\r\n"
                          << "Transfer-Encoding: chunked\r\n"
                          << "Content-Type: application/vnd.remote-exec.transfer-stream.v2\r\n"
                          << "x-remote-exec-transfer-stream-version: 2\r\n"
                          << "x-remote-exec-source-type: file\r\n"
                          << "x-remote-exec-destination-path: "
                          << encoded_destination_path_header(outside / "imported.txt") << "\r\n"
                          << "x-remote-exec-overwrite: replace\r\n"
                          << "x-remote-exec-create-parent: true\r\n"
                          << "x-remote-exec-symlink-mode: preserve\r\n"
                          << "x-remote-exec-compression: none\r\n"
                          << "\r\n"
                          << chunked_body(framed_transfer_body(archive));
    const std::string denied_import_response = run_single_request(sandbox_state, denied_import_request.str());
    TEST_ASSERT(denied_import_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    TEST_ASSERT(Json::parse(response_body(denied_import_response)).at("code").get<std::string>() == "sandbox_denied");
    TEST_ASSERT(!fs::exists(outside / "imported.txt"));
}
