#include "test_assert.h"
#include <climits>
#include <limits>
#include <string>

#include <stdexcept>
#include <utility>

#include "http_request.h"
#include "server_transport.h"
#include "test_socket_pair.h"

int main() {
    bool rejected_invalid_timeout_socket = false;
    try {
        set_socket_timeout_ms(INVALID_SOCKET, 1000UL);
    } catch (const std::runtime_error&) {
        rejected_invalid_timeout_socket = true;
    }
    TEST_ASSERT(rejected_invalid_timeout_socket);

    TEST_ASSERT(bounded_socket_io_size(0U) == 0U);
    TEST_ASSERT(bounded_socket_io_size(1U) == 1U);
    TEST_ASSERT(bounded_socket_io_size(static_cast<std::size_t>(INT_MAX)) == static_cast<std::size_t>(INT_MAX));
    TEST_ASSERT(bounded_socket_io_size(static_cast<std::size_t>(INT_MAX) + 1U) == static_cast<std::size_t>(INT_MAX));
    TEST_ASSERT(bounded_socket_io_size(std::numeric_limits<std::size_t>::max()) == static_cast<std::size_t>(INT_MAX));

    ConnectedSocketPair sockets = make_connected_socket_pair();
    UniqueSocket reader(std::move(sockets.first));
    UniqueSocket writer(std::move(sockets.second));

    const std::string raw = "POST /v1/transfer/import HTTP/1.1\r\n"
                            "Host: cpp-daemon\r\n"
                            "Transfer-Encoding: chunked\r\n"
                            "\r\n"
                            "5\r\n"
                            "hello\r\n"
                            "6\r\n"
                            " world\r\n"
                            "0\r\n"
                            "\r\n";

    send_all(writer.get(), raw);
    writer.reset();

    HttpRequestHead head;
    TEST_ASSERT(try_read_http_request_head(reader.get(), 65536, &head));

    const HttpRequest request = parse_http_request_head(head.raw_headers);
    const HttpRequestBodyFraming framing = request_body_framing_from_headers(request.headers);
    HttpRequestBodyStream body(reader.get(), head.initial_body, framing, 1024);

    std::string decoded;
    char buffer[4];
    for (;;) {
        const std::size_t received = body.read(buffer, sizeof(buffer));
        if (received == 0U) {
            break;
        }
        decoded.append(buffer, received);
    }

    TEST_ASSERT(request.path == "/v1/transfer/import");
    TEST_ASSERT(request.header("transfer-encoding") == "chunked");
    TEST_ASSERT(decoded == "hello world");

    return 0;
}
