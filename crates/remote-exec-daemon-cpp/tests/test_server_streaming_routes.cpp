#include "test_server_streaming_shared.h"

#include <algorithm>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <sstream>

#include "codec/base64_codec.h"
#include "policy/path_policy.h"
#include "test_socket_pair.h"
#include "test_tar_helpers.h"

namespace tar = test_tar;

static std::size_t response_content_length(const std::string& header_block) {
    const std::string marker = "\r\nContent-Length: ";
    const std::size_t start = header_block.find(marker);
    TEST_ASSERT(start != std::string::npos);
    const std::size_t value_start = start + marker.size();
    const std::size_t value_end = header_block.find("\r\n", value_start);
    TEST_ASSERT(value_end != std::string::npos);
    return static_cast<std::size_t>(
        std::strtoull(header_block.substr(value_start, value_end - value_start).c_str(), NULL, 10)
    );
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

static void send_request_and_close_writer(SOCKET socket, const std::string& request) {
    send_all(socket, request);
#ifdef _WIN32
    shutdown(socket, SD_SEND);
#else
    shutdown(socket, SHUT_WR);
#endif
}

void assert_first_request_callback_waits_for_request(TestHttpConnectionHarness& harness) {
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
    std::atomic<bool> first_request(false);
    std::thread server_thread(
        [&harness, &first_request](SOCKET socket) {
            handle_client(*harness.connection, UniqueSocket(socket), [&first_request]() {
                first_request.store(true);
            });
        },
        server_socket.release()
    );

    platform::sleep_ms(50UL);
    TEST_ASSERT(!first_request.load());
    send_all(
        client_socket.get(),
        "POST /v1/health HTTP/1.1\r\n"
        "Connection: close\r\n"
        "Content-Length: 0\r\n"
        "\r\n"
    );
    TEST_ASSERT(test_assert::wait_until_true(first_request, 1000UL));
    const std::string response = read_all_from_socket(client_socket.get());
    TEST_ASSERT(response.find("HTTP/1.1 200 OK\r\n") == 0);
    server_thread.join();
}

static std::string response_body(const std::string& response) {
    const std::size_t header_end = response.find("\r\n\r\n");
    TEST_ASSERT(header_end != std::string::npos);
    return response.substr(header_end + 4);
}

static void assert_json_response_code(
    const std::string& response,
    const std::string& status_line,
    const std::string& code
) {
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

static std::string single_file_tar_body(const std::string& archive) {
    TEST_ASSERT(archive.size() >= 512);
    const char* header = archive.data();
    std::size_t path_length = 0;
    while (path_length < 100 && header[path_length] != '\0') {
        ++path_length;
    }
    TEST_ASSERT(std::string(header, path_length) == ".remote-exec-file");
    TEST_ASSERT(header[156] == '0');
    const std::uint64_t size = tar::parse_octal_value(header + 124, 12);
    TEST_ASSERT(512 + static_cast<std::size_t>(size) <= archive.size());
    return archive.substr(512, static_cast<std::size_t>(size));
}

static std::string tar_with_single_file(const std::string& body) {
    std::string archive;
    tar::append_tar_file(archive, ".remote-exec-file", body);
    tar::finalize_tar(archive);
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

static Json decode_framed_transfer_error(const std::string& body) {
    TEST_ASSERT(
        body.compare(
            0,
            server_contract::TRANSFER_STREAM_PREFACE_LEN,
            server_contract::TRANSFER_STREAM_PREFACE
        )
        == 0
    );
    std::size_t offset = server_contract::TRANSFER_STREAM_PREFACE_LEN;
    for (;;) {
        TEST_ASSERT(offset + server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN <= body.size());
        const unsigned char frame_type = static_cast<unsigned char>(body[offset]);
        const std::uint64_t payload_len = tar::read_u64_be(body, offset + 4U);
        offset += server_contract::TRANSFER_STREAM_FRAME_HEADER_LEN;
        TEST_ASSERT(payload_len <= static_cast<std::uint64_t>(body.size() - offset));
        const std::string payload = body.substr(offset, static_cast<std::size_t>(payload_len));
        offset += static_cast<std::size_t>(payload_len);
        if (frame_type == 0x03U) {
            return Json::parse(payload);
        }
    }
}

static std::string run_single_request(
    TestHttpConnectionHarness& harness,
    const std::string& request
) {
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
    send_request_and_close_writer(client_socket.get(), request);
    handle_client(harness, std::move(server_socket));
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
            << extra_headers << "\r\n"
            << payload;
    return request.str();
}

static std::string streaming_import_request(
    const fs::path& destination,
    const std::string& archive
) {
    std::ostringstream request;
    request << "POST /v1/transfer/import HTTP/1.1\r\n"
            << "Transfer-Encoding: chunked\r\n"
            << "Content-Type: " << server_contract::TRANSFER_STREAM_CONTENT_TYPE << "\r\n"
            << server_contract::TRANSFER_STREAM_VERSION_HEADER << ": "
            << server_contract::TRANSFER_STREAM_VERSION_VALUE << "\r\n"
            << server_contract::TRANSFER_SOURCE_TYPE_HEADER << ": file\r\n"
            << server_contract::TRANSFER_DESTINATION_PATH_HEADER << ": "
            << tar::encoded_destination_path_header(destination) << "\r\n"
            << server_contract::TRANSFER_OVERWRITE_HEADER << ": replace\r\n"
            << server_contract::TRANSFER_CREATE_PARENT_HEADER << ": true\r\n"
            << server_contract::TRANSFER_SYMLINK_MODE_HEADER << ": preserve\r\n"
            << server_contract::TRANSFER_COMPRESSION_HEADER << ": none\r\n"
            << "\r\n"
            << chunked_body(tar::framed_transfer_body(archive));
    return request.str();
}

static std::string streaming_export_request(const std::string& body) {
    std::ostringstream request;
    request << "POST /v1/transfer/export HTTP/1.1\r\n"
            << "Content-Length: " << body.size() << "\r\n"
            << server_contract::TRANSFER_STREAM_VERSION_HEADER << ": "
            << server_contract::TRANSFER_STREAM_VERSION_VALUE << "\r\n"
            << "\r\n"
            << body;
    return request.str();
}

static void assert_persistent_json_requests_reuse_socket(TestHttpConnectionHarness& harness) {
    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
    std::thread server_thread(
        [&harness](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(harness, std::move(owned_socket));
        },
        server_socket.release()
    );

    send_all(client_socket.get(), json_post_request("/v1/health", Json::object()));
    const std::string first_response =
        read_content_length_response_from_socket(client_socket.get());
    TEST_ASSERT(first_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(first_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(Json::parse(response_body(first_response)).at("status").get<std::string>() == "ok");

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
    TEST_ASSERT(second_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(second_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(
        Json::parse(response_body(second_response)).at("target").get<std::string>() == "cpp-test"
    );

    char extra = '\0';
    TEST_ASSERT(recv(client_socket.get(), &extra, 1, 0) == 0);
    server_thread.join();
}

static void assert_http_auth_and_rejection_paths(
    TestHttpConnectionHarness& harness,
    const fs::path& root
) {
    const std::string missing_auth_response =
        run_single_request(harness, json_post_request("/v1/health", Json::object()));
    assert_json_response_code(
        missing_auth_response,
        "HTTP/1.1 401 Unauthorized\r\n",
        "unauthorized"
    );
    TEST_ASSERT(missing_auth_response.find("WWW-Authenticate: Bearer\r\n") != std::string::npos);

    const std::string wrong_auth_response = run_single_request(
        harness,
        json_post_request_with_extra_headers(
            "/v1/health",
            Json::object(),
            "Authorization: Bearer wrong-secret\r\n"
        )
    );
    assert_json_response_code(wrong_auth_response, "HTTP/1.1 401 Unauthorized\r\n", "unauthorized");

    const std::string ok_response = run_single_request(
        harness,
        json_post_request_with_extra_headers(
            "/v1/health",
            Json::object(),
            "Authorization: Bearer shared-secret\r\n"
        )
    );
    TEST_ASSERT(ok_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(Json::parse(response_body(ok_response)).at("status").get<std::string>() == "ok");

    TestHttpConnectionHarness unauthenticated(root);

    const std::string get_response = run_single_request(
        unauthenticated,
        "GET /v1/health HTTP/1.1\r\n"
        "Content-Length: 0\r\n"
        "\r\n"
    );
    assert_json_response_code(
        get_response,
        "HTTP/1.1 405 Method Not Allowed\r\n",
        "method_not_allowed"
    );

    const std::string not_found_response =
        run_single_request(unauthenticated, json_post_request("/v1/not-found", Json::object()));
    assert_json_response_code(not_found_response, "HTTP/1.1 404 Not Found\r\n", "not_found");
}

static void assert_http_request_limits_through_connection_path(const fs::path& root) {
    TestHttpConnectionHarness header_limit(root);
    header_limit.state.config.max_request_header_bytes = 48U;
    header_limit.refresh_context();

    std::ostringstream oversized_header_request;
    oversized_header_request << "POST /v1/health HTTP/1.1\r\n"
                             << "X-Too-Large: " << std::string(80, 'x') << "\r\n"
                             << "\r\n";
    const std::string header_limit_response =
        run_single_request(header_limit, oversized_header_request.str());
    assert_json_response_code(header_limit_response, "HTTP/1.1 400 Bad Request\r\n", "bad_request");
    TEST_ASSERT(
        response_body(header_limit_response).find("http request headers too large")
        != std::string::npos
    );

    TestHttpConnectionHarness content_length_limit(root);
    content_length_limit.state.config.max_request_body_bytes = 4U;
    content_length_limit.refresh_context();

    const std::string content_length_limit_response = run_single_request(
        content_length_limit,
        "POST /v1/health HTTP/1.1\r\n"
        "Content-Length: 5\r\n"
        "\r\n"
        "12345"
    );
    assert_json_response_code(
        content_length_limit_response,
        "HTTP/1.1 400 Bad Request\r\n",
        "bad_request"
    );
    TEST_ASSERT(
        response_body(content_length_limit_response).find("http request body too large")
        != std::string::npos
    );

    TestHttpConnectionHarness chunked_limit(root);
    chunked_limit.state.config.max_request_body_bytes = 4U;
    chunked_limit.refresh_context();

    const std::string chunked_limit_response = run_single_request(
        chunked_limit,
        "POST /v1/health HTTP/1.1\r\n"
        "Transfer-Encoding: chunked\r\n"
        "\r\n"
        "5\r\n"
        "12345\r\n"
        "0\r\n"
        "\r\n"
    );
    assert_json_response_code(
        chunked_limit_response,
        "HTTP/1.1 400 Bad Request\r\n",
        "bad_request"
    );
    TEST_ASSERT(
        response_body(chunked_limit_response).find("http request body too large")
        != std::string::npos
    );
}

static void assert_streaming_import_ignores_generic_http_body_limit(const fs::path& root) {
    TestHttpConnectionHarness harness(root);
    harness.state.config.max_request_body_bytes = 32U;
    harness.state.config.transfer_limits.max_archive_bytes = 4096U;
    harness.state.config.transfer_limits.max_entry_bytes = 4096U;
    harness.refresh_context();

    const std::string archive = tar_with_single_file("stream body exceeds generic http limit");
    TEST_ASSERT(
        tar::framed_transfer_body(archive).size() > harness.state.config.max_request_body_bytes
    );

    const fs::path imported_path = root / "small-http-limit-import.txt";
    const std::string request = streaming_import_request(imported_path, archive);

    const std::string response = run_single_request(harness, request);
    TEST_ASSERT(response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(fs::read_file_bytes(imported_path) == "stream body exceeds generic http limit");
}

static void assert_streaming_import_failure_closes_connection_with_unread_body(const fs::path& root
) {
    TestHttpConnectionHarness harness(root);
    harness.state.config.transfer_limits.max_archive_bytes = 16U;
    harness.state.config.transfer_limits.max_entry_bytes = 4096U;
    harness.refresh_context();

    const std::string archive = tar_with_single_file(std::string(128U, 'x'));
    const std::string framed = tar::framed_transfer_body(archive);
    const fs::path imported_path = root / "failed-import.txt";

    const std::string full_request = streaming_import_request(imported_path, archive);
    const std::size_t request_head_end = full_request.find("\r\n\r\n") + 4U;
    const std::string request_head = full_request.substr(0, request_head_end);

    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket server_socket(std::move(sockets.first));
    UniqueSocket client_socket(std::move(sockets.second));
    std::thread server_thread(
        [&harness](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(harness, std::move(owned_socket));
        },
        server_socket.release()
    );

    send_all(client_socket.get(), request_head);

    const std::size_t split_offset = 24U;
    send_all(client_socket.get(), chunked_data_only(framed.substr(0U, split_offset)));

    send_all(client_socket.get(), chunked_body(framed.substr(split_offset)));

    const std::string response = read_content_length_response_from_socket(client_socket.get());
    assert_json_response_code(response, "HTTP/1.1 400 Bad Request\r\n", "transfer_failed");

    assert_socket_closed_within(client_socket.get(), 5000UL);
    TEST_ASSERT(!fs::exists(imported_path));
    server_thread.join();
}

void assert_http_streaming_routes(TestHttpConnectionHarness& harness, const fs::path& root) {
    assert_persistent_json_requests_reuse_socket(harness);

    TestHttpConnectionHarness auth(root);
    auth.state.config.http_auth_bearer_token = "shared-secret";
    auth.refresh_context();
    assert_http_auth_and_rejection_paths(auth, root);
    assert_http_request_limits_through_connection_path(root);
    assert_streaming_import_ignores_generic_http_body_limit(root);
    assert_streaming_import_failure_closes_connection_with_unread_body(root);

    const std::string archive = tar_with_single_file("streamed import");
    const fs::path imported_path = root / "imported.txt";
    const std::string import_request = streaming_import_request(imported_path, archive);

    const std::string import_response = run_single_request(harness, import_request);
    TEST_ASSERT(import_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(fs::read_file_bytes(imported_path) == "streamed import");

    const fs::path export_path = root / "export.txt";
    fs::write_file_bytes(export_path, "streamed export");
    const std::string export_body = Json{{"path", export_path.string()}}.dump();
    const std::string export_request = streaming_export_request(export_body);

    const std::string export_response = run_single_request(harness, export_request);
    TEST_ASSERT(export_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(export_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos);
    TEST_ASSERT(export_response.find("Connection: close\r\n") == std::string::npos);
    TEST_ASSERT(export_response.find("Content-Length:") == std::string::npos);
    TEST_ASSERT(export_response.find("x-remote-exec-source-type: file\r\n") != std::string::npos);
    TEST_ASSERT(
        single_file_tar_body(
            tar::decode_framed_transfer_archive(decode_chunked_response_body(export_response))
        )
        == "streamed export"
    );

    const fs::path sandbox_root = root / "sandbox";
    const fs::path read_allowed = sandbox_root / "read";
    const fs::path write_allowed = sandbox_root / "write";
    const fs::path outside = sandbox_root / "outside";
    fs::create_directories(read_allowed);
    fs::create_directories(write_allowed);
    fs::create_directories(outside);
    fs::write_file_bytes(outside / "outside.txt", "outside");

    TestHttpConnectionHarness sandbox(root);
    sandbox.state.config.sandbox_configured = true;
    sandbox.state.config.sandbox.read.allow.push_back(read_allowed.string());
    sandbox.state.config.sandbox.write.allow.push_back(write_allowed.string());
    enable_test_daemon_sandbox(sandbox.state);
    sandbox.refresh_context();

    const std::string denied_export_body =
        Json{{"path", (outside / "outside.txt").string()}}.dump();
    const std::string denied_export_response =
        run_single_request(sandbox, streaming_export_request(denied_export_body));
    TEST_ASSERT(denied_export_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    TEST_ASSERT(
        Json::parse(response_body(denied_export_response)).at("code").get<std::string>()
        == "sandbox_denied"
    );

    const fs::path recursive_read_root = read_allowed / "recursive";
    const fs::path recursive_denied = recursive_read_root / "secret";
    fs::create_directories(recursive_denied);
    fs::write_file_bytes(recursive_read_root / "visible.txt", "visible");
    fs::write_file_bytes(recursive_denied / "hidden.txt", "hidden");

    TestHttpConnectionHarness recursive_deny(root);
    recursive_deny.state.config.sandbox_configured = true;
    recursive_deny.state.config.sandbox.read.allow.push_back(read_allowed.string());
    recursive_deny.state.config.sandbox.read.deny.push_back(recursive_denied.string());
    enable_test_daemon_sandbox(recursive_deny.state);
    recursive_deny.refresh_context();

    const std::string recursive_deny_body = Json{{"path", recursive_read_root.string()}}.dump();
    const std::string recursive_deny_response =
        run_single_request(recursive_deny, streaming_export_request(recursive_deny_body));
    TEST_ASSERT(recursive_deny_response.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(
        recursive_deny_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos
    );
    TEST_ASSERT(
        decode_framed_transfer_error(decode_chunked_response_body(recursive_deny_response))
            .at("code")
            .get<std::string>()
        == "sandbox_denied"
    );

    const std::string denied_import_response =
        run_single_request(sandbox, streaming_import_request(outside / "imported.txt", archive));
    TEST_ASSERT(denied_import_response.find("HTTP/1.1 400 Bad Request\r\n") == 0);
    TEST_ASSERT(
        Json::parse(response_body(denied_import_response)).at("code").get<std::string>()
        == "sandbox_denied"
    );
    TEST_ASSERT(!fs::exists(outside / "imported.txt"));
}
