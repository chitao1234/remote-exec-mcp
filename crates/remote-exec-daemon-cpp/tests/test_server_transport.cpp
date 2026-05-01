#include <cassert>
#include <string>

#include <sys/socket.h>

#include "http_request.h"
#include "server_transport.h"

int main() {
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

    const std::string received = read_http_request(reader.get(), 65536, 1024);
    const HttpRequest request = parse_http_request(received);
    assert(request.path == "/v1/transfer/import");
    assert(request.header("transfer-encoding") == "chunked");
    assert(request.body == "hello world");

    return 0;
}
