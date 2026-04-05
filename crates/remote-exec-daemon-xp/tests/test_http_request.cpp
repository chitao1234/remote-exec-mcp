#include <cassert>
#include <string>

#include "http_helpers.h"
#include "http_request.h"

int main() {
    const std::string raw =
        "POST /v1/exec/start HTTP/1.1\r\n"
        "Content-Length: 13\r\n"
        "X-Test:   Value  \r\n"
        "\r\n"
        "{\"cmd\":\"dir\"}";

    const HttpRequest request = parse_http_request(raw);
    assert(request.method == "POST");
    assert(request.path == "/v1/exec/start");
    assert(request.header("content-length") == "13");
    assert(request.header("x-test") == "Value");
    assert(request.body == "{\"cmd\":\"dir\"}");

    bool rejected = false;
    try {
        (void)parse_http_request("invalid");
    } catch (...) {
        rejected = true;
    }
    assert(rejected);

    return 0;
}
