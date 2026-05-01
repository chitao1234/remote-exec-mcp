#include <cassert>
#include <string>

#include "http_helpers.h"
#include "http_request.h"

int main() {
    const std::string raw =
        "POST /v1/exec/start HTTP/1.1\r\n"
        "Authorization: Bearer shared-secret\r\n"
        "Content-Length: 13\r\n"
        "X-Test:   Value  \r\n"
        "\r\n"
        "{\"cmd\":\"dir\"}";

    const HttpRequest request = parse_http_request(raw);
    assert(request.method == "POST");
    assert(request.path == "/v1/exec/start");
    assert(request.header("content-length") == "13");
    assert(request.header("x-test") == "Value");
    assert(request.header("authorization") == "Bearer shared-secret");
    assert(request.body == "{\"cmd\":\"dir\"}");
    assert(request_has_bearer_auth(request, "shared-secret"));
    assert(!request_has_bearer_auth(request, "wrong-secret"));

    const std::string chunked_raw =
        "POST /v1/exec/start HTTP/1.1\r\n"
        "Transfer-Encoding: chunked\r\n"
        "\r\n"
        "7;source=test\r\n"
        "{\"cmd\":\r\n"
        "6\r\n"
        "\"dir\"}\r\n"
        "0\r\n"
        "X-Transfer-Warning: ignored\r\n"
        "\r\n";

    const HttpRequest chunked_request = parse_http_request(chunked_raw);
    assert(chunked_request.header("transfer-encoding") == "chunked");
    assert(chunked_request.body == "{\"cmd\":\"dir\"}");

    bool bad_chunk_rejected = false;
    try {
        (void)parse_http_request(
            "POST /v1/exec/start HTTP/1.1\r\n"
            "Transfer-Encoding: chunked\r\n"
            "\r\n"
            "not-hex\r\n"
            "body\r\n"
            "0\r\n"
            "\r\n"
        );
    } catch (...) {
        bad_chunk_rejected = true;
    }
    assert(bad_chunk_rejected);

    HttpResponse unauthorized;
    write_bearer_auth_challenge(unauthorized);
    assert(unauthorized.status == 401);
    assert(unauthorized.headers["WWW-Authenticate"] == "Bearer");
    assert(unauthorized.body.find("\"code\":\"unauthorized\"") != std::string::npos);

    bool rejected = false;
    try {
        (void)parse_http_request("invalid");
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    return 0;
}
