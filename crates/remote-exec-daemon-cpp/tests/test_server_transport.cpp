#include <cassert>
#include <string>

#include <stdexcept>
#include <sys/socket.h>

#include "http_request.h"
#include "server_transport.h"

int main() {
    bool rejected_invalid_timeout_socket = false;
    try {
        set_socket_timeout_ms(INVALID_SOCKET, 1000UL);
    } catch (const std::runtime_error&) {
        rejected_invalid_timeout_socket = true;
    }
    assert(rejected_invalid_timeout_socket);

    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket reader(sockets[0]);
    UniqueSocket writer(sockets[1]);

    const std::string raw =
        "POST /v1/transfer/import HTTP/1.1\r\n"
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
    assert(try_read_http_request_head(reader.get(), 65536, &head));

    const HttpRequest request = parse_http_request_head(head.raw_headers);
    const HttpRequestBodyFraming framing =
        request_body_framing_from_headers(request.headers);
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

    assert(request.path == "/v1/transfer/import");
    assert(request.header("transfer-encoding") == "chunked");
    assert(decoded == "hello world");

    return 0;
}
