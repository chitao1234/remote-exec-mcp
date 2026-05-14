#include "test_assert.h"
#include <string>

#include "http_helpers.h"
#include "http_request.h"

static void assert_rejects(const std::string& raw) {
    bool rejected = false;
    try {
        (void)parse_http_request(raw);
    } catch (const HttpParseError&) {
        rejected = true;
    }
    TEST_ASSERT(rejected);
}

int main() {
    const std::string raw = "POST /v1/exec/start HTTP/1.1\r\n"
                            "Authorization: Bearer shared-secret\r\n"
                            "Content-Length: 13\r\n"
                            "X-Test:   Value  \r\n"
                            "\r\n"
                            "{\"cmd\":\"dir\"}";

    const HttpRequest request = parse_http_request(raw);
    TEST_ASSERT(request.method == "POST");
    TEST_ASSERT(request.path == "/v1/exec/start");
    TEST_ASSERT(request.header("content-length") == "13");
    TEST_ASSERT(request.header("x-test") == "Value");
    TEST_ASSERT(request.header("authorization") == "Bearer shared-secret");
    TEST_ASSERT(request.body == "{\"cmd\":\"dir\"}");
    TEST_ASSERT(request_has_bearer_auth(request, "shared-secret"));
    TEST_ASSERT(!request_has_bearer_auth(request, "wrong-secret"));
    TEST_ASSERT(!request_has_bearer_auth(request, "shared"));
    TEST_ASSERT(!request_has_bearer_auth(request, "shared-secret-extra"));

    HttpRequest wrong_prefix = request;
    wrong_prefix.headers["authorization"] = "Basic shared-secret";
    TEST_ASSERT(!request_has_bearer_auth(wrong_prefix, "shared-secret"));

    HttpRequest shorter_value = request;
    shorter_value.headers["authorization"] = "Bearer shared";
    TEST_ASSERT(!request_has_bearer_auth(shorter_value, "shared-secret"));

    HttpRequest longer_value = request;
    longer_value.headers["authorization"] = "Bearer shared-secret-extra";
    TEST_ASSERT(!request_has_bearer_auth(longer_value, "shared-secret"));

    const std::string chunked_raw = "POST /v1/exec/start HTTP/1.1\r\n"
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
    TEST_ASSERT(chunked_request.header("transfer-encoding") == "chunked");
    TEST_ASSERT(chunked_request.body == "{\"cmd\":\"dir\"}");

    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Transfer-Encoding: chunked\r\n"
                   "\r\n"
                   "not-hex\r\n"
                   "body\r\n"
                   "0\r\n"
                   "\r\n");

    HttpResponse unauthorized;
    write_bearer_auth_challenge(unauthorized);
    TEST_ASSERT(unauthorized.status == 401);
    TEST_ASSERT(unauthorized.headers["WWW-Authenticate"] == "Bearer");
    TEST_ASSERT(unauthorized.body.find("\"code\":\"unauthorized\"") != std::string::npos);

    HttpResponse ok;
    write_json(ok, Json{{"status", "ok"}});
    const std::string rendered = render_http_response(ok);
    TEST_ASSERT(rendered.find("HTTP/1.1 200 OK\r\n") == 0);
    TEST_ASSERT(rendered.find("Content-Type: application/json\r\n") != std::string::npos);
    TEST_ASSERT(rendered.find("Content-Length: ") != std::string::npos);
    TEST_ASSERT(rendered.find("Connection: close\r\n") == std::string::npos);

    assert_rejects("invalid");
    assert_rejects("POST /v1/exec/start\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.1 extra\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/2.0\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.0\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Bad Header\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   ": no-name\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Bad Header: value\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "X-Test: one\r\n"
                   "x-test: two\r\n"
                   "\r\n");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Content-Length: 5\r\n"
                   "\r\n"
                   "too long");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Content-Length: 8\r\n"
                   "\r\n"
                   "short");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Transfer-Encoding: chunked\r\n"
                   "\r\n"
                   "0\r\n"
                   "\r\n"
                   "extra");
    assert_rejects("POST /v1/exec/start HTTP/1.1\r\n"
                   "Transfer-Encoding: chunked\r\n"
                   "\r\n"
                   "0\r\n"
                   "bad trailer\r\n"
                   "\r\n");

    return 0;
}
