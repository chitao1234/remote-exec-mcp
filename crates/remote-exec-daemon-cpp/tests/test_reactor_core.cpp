#include "test_assert.h"

#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <thread>

#ifndef _WIN32
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/socket.h>
#include <unistd.h>
#endif

#include "core/byte_buffer.h"
#include "core/event_loop.h"
#include "core/json_value.h"
#include "platform/platform.h"
#include "platform/socket.h"
#include "runtime/reactor_server.h"

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

Response request(unsigned short port, const char* path, const std::string& body) {
    const int fd = connect_local(port);
    std::string request_text = std::string("POST ") + path + " HTTP/1.1\r\n";
    request_text += "Host: 127.0.0.1\r\n";
    request_text += "Connection: close\r\n";
    request_text += "Content-Length: " + std::to_string(body.size()) + "\r\n\r\n";
    request_text += body;
    send_all_text(fd, request_text);
    const Response response = read_response(fd);
    close_socket(fd);
    return response;
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

    const Response unmigrated = request(port, "/v1/exec/start", "{\"cmd\":\"echo hi\"}");
    TEST_ASSERT(unmigrated.status == 501);
    TEST_ASSERT(unmigrated.body.find("\"not_migrated\"") != std::string::npos);

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
#endif
    return 0;
}
