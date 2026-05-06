# Daemon HTTP Keepalive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Enable true persistent HTTP/1.1 broker-to-daemon connection reuse and remove forced `Connection: close` behavior across Rust and C++ daemon communication.

**Architecture:** Let the Rust broker's `reqwest::Client` own connection pooling by removing the forced close request header. Keep Rust daemon serving through Hyper HTTP/1.1, add HTTP/1.1-only validation, and make the C++ daemon process multiple sequential HTTP/1.1 requests per socket with explicit close-on-error and close-on-`Connection: close` behavior.

**Tech Stack:** Rust 2024, `reqwest`, Axum/Hyper HTTP/1.1, Tokio integration tests, C++11 daemon runtime, C++17 host tests, POSIX socketpair-based C++ tests.

---

## File Structure

- `crates/remote-exec-broker/src/daemon_client.rs`
  - Owns broker-to-daemon `reqwest::Client` construction and request headers.
  - Add focused unit tests for daemon request headers.
  - Remove `Connection: close` injection from all daemon RPC paths.

- `crates/remote-exec-daemon/src/http/version.rs`
  - New focused Axum middleware that rejects non-HTTP/1.1 daemon RPC requests.

- `crates/remote-exec-daemon/src/http/mod.rs`
  - Expose the new `version` middleware module.

- `crates/remote-exec-daemon/src/http/routes.rs`
  - Attach HTTP-version validation to the daemon router.

- `crates/remote-exec-daemon/tests/health.rs`
  - Add raw-socket coverage for `HTTP/1.0` rejection.

- `crates/remote-exec-daemon-cpp/src/http_request.cpp`
  - Make the C++ parser accept only `HTTP/1.1`.

- `crates/remote-exec-daemon-cpp/src/http_helpers.cpp`
  - Stop injecting `Connection: close` in normal responses.

- `crates/remote-exec-daemon-cpp/include/server_transport.h`
  - Add a clean-EOF-aware request-head read API for persistent connections.

- `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
  - Implement clean EOF detection before a new keepalive request starts.

- `crates/remote-exec-daemon-cpp/include/server.h`
  - Rename the public socket handler from one-shot naming to persistent-client naming.

- `crates/remote-exec-daemon-cpp/src/server.cpp`
  - Remove streaming export close header.
  - Add `Connection: close` token detection.
  - Loop per accepted socket until EOF, requested close, malformed input, or send failure.

- `crates/remote-exec-daemon-cpp/tests/test_http_request.cpp`
  - Add parser and normal-response header coverage.

- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
  - Add persistent sequential request coverage and streaming export header coverage.

- `README.md`
  - Document that broker-daemon RPC is HTTP/1.1-only and can reuse persistent connections.

- `crates/remote-exec-daemon-cpp/README.md`
  - Document C++ daemon HTTP/1.1-only, persistent sequential requests, and no pipelining.

---

### Task 1: Broker Request Headers

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Test/Verify: `cargo test -p remote-exec-broker daemon_request --lib`

**Testing approach:** TDD  
Reason: The behavior seam is a private request builder with a direct observable header map, so a small unit test can fail before the implementation and pass after removing the forced header.

- [ ] **Step 1: Add failing broker request-header tests**

Add this `#[cfg(test)]` module at the end of `crates/remote-exec-broker/src/daemon_client.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(authorization: Option<HeaderValue>) -> DaemonClient {
        DaemonClient {
            client: reqwest::Client::builder().build().unwrap(),
            target_name: "builder-a".to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            authorization,
        }
    }

    #[test]
    fn daemon_request_does_not_force_connection_close() {
        let request = test_client(None)
            .request("/v1/target-info")
            .build()
            .unwrap();

        assert!(
            request.headers().get(reqwest::header::CONNECTION).is_none(),
            "broker daemon client should let reqwest manage persistent connections"
        );
    }

    #[test]
    fn daemon_request_still_applies_authorization_header() {
        let request = test_client(Some(HeaderValue::from_static("Bearer shared-secret")))
            .request("/v1/target-info")
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer shared-secret")
        );
        assert!(request.headers().get(reqwest::header::CONNECTION).is_none());
    }
}
```

- [ ] **Step 2: Run the focused failing test**

Run: `cargo test -p remote-exec-broker daemon_request --lib`

Expected: `daemon_request_does_not_force_connection_close` fails because `DaemonClient::request` still adds `Connection: close`.

- [ ] **Step 3: Remove the forced broker close header**

Change the import at the top of `crates/remote-exec-broker/src/daemon_client.rs` from:

```rust
use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HeaderValue};
```

to:

```rust
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, HeaderValue};
```

Change `DaemonClient::request` from:

```rust
    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(CONNECTION, "close");
        if let Some(authorization) = &self.authorization {
            request = request.header(AUTHORIZATION, authorization.clone());
        }
        request
    }
```

to:

```rust
    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut request = self.client.post(format!("{}{}", self.base_url, path));
        if let Some(authorization) = &self.authorization {
            request = request.header(AUTHORIZATION, authorization.clone());
        }
        request
    }
```

- [ ] **Step 4: Run the focused broker verification**

Run: `cargo test -p remote-exec-broker daemon_request --lib`

Expected: both `daemon_request_*` tests pass.

- [ ] **Step 5: Checkpoint the task without committing**

Run: `git diff --check -- crates/remote-exec-broker/src/daemon_client.rs && git status --short`

Expected: no whitespace errors; `crates/remote-exec-broker/src/daemon_client.rs` is listed as modified. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 2: Rust Daemon HTTP/1.1 Validation

**Files:**
- Create: `crates/remote-exec-daemon/src/http/version.rs`
- Modify: `crates/remote-exec-daemon/src/http/mod.rs`
- Modify: `crates/remote-exec-daemon/src/http/routes.rs`
- Modify: `crates/remote-exec-daemon/tests/health.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test health`

**Testing approach:** TDD  
Reason: HTTP-version rejection is externally visible through a raw HTTP request, and the test can prove `HTTP/1.0` is rejected before adding middleware.

- [ ] **Step 1: Add a failing raw HTTP/1.0 rejection test**

In `crates/remote-exec-daemon/tests/health.rs`, add these imports near the existing imports:

```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt};
```

Add this helper near `startup_validation_config`:

```rust
async fn raw_http_request(addr: SocketAddr, request: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}
```

Add this test after `target_info_is_available_over_plain_http`:

```rust
#[tokio::test]
async fn daemon_rejects_http_1_0_rpc_requests() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;

    let response = raw_http_request(
        fixture.addr,
        "POST /v1/target-info HTTP/1.0\r\nContent-Length: 2\r\n\r\n{}",
    )
    .await;

    assert!(
        response.starts_with("HTTP/1.0 400 Bad Request\r\n")
            || response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{response}"
    );
    assert!(response.contains("\"code\":\"bad_request\""), "{response}");
    assert!(
        response.contains("only HTTP/1.1 is supported"),
        "{response}"
    );
}
```

- [ ] **Step 2: Run the focused failing daemon test**

Run: `cargo test -p remote-exec-daemon --test health daemon_rejects_http_1_0_rpc_requests`

Expected: the test fails because the daemon currently accepts `HTTP/1.0` and returns `200 OK`.

- [ ] **Step 3: Add HTTP/1.1-only middleware**

Create `crates/remote-exec-daemon/src/http/version.rs`:

```rust
use axum::extract::Request;
use axum::http::{StatusCode, Version};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use remote_exec_proto::rpc::RpcErrorBody;

pub async fn require_http_11(request: Request, next: Next) -> Response {
    if request.version() == Version::HTTP_11 {
        return next.run(request).await;
    }

    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code: "bad_request".to_string(),
            message: "only HTTP/1.1 is supported".to_string(),
        }),
    )
        .into_response()
}
```

Update `crates/remote-exec-daemon/src/http/mod.rs` from:

```rust
pub mod auth;
pub mod request_log;
pub mod routes;
```

to:

```rust
pub mod auth;
pub mod request_log;
pub mod routes;
pub mod version;
```

In `crates/remote-exec-daemon/src/http/routes.rs`, add the version middleware to the router chain. The bottom of `router` should become:

```rust
        .layer(middleware::from_fn(super::version::require_http_11))
        .layer(middleware::from_fn_with_state(
            daemon_config,
            super::auth::require_http_auth,
        ))
        .with_state(state)
        .layer(middleware::from_fn(super::request_log::log_http_request))
}
```

- [ ] **Step 4: Run focused Rust daemon verification**

Run: `cargo test -p remote-exec-daemon --test health`

Expected: all tests in `health.rs` pass, including `target_info_is_available_over_plain_http` and `daemon_rejects_http_1_0_rpc_requests`.

- [ ] **Step 5: Checkpoint the task without committing**

Run: `git diff --check -- crates/remote-exec-daemon/src/http/version.rs crates/remote-exec-daemon/src/http/mod.rs crates/remote-exec-daemon/src/http/routes.rs crates/remote-exec-daemon/tests/health.rs && git status --short`

Expected: no whitespace errors; the new middleware file and modified daemon files are listed. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 3: C++ Parser and Header Characterization Tests

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_http_request.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-http-request`

**Testing approach:** TDD  
Reason: C++ parser and response rendering have direct unit seams, and both current behaviors should fail before implementation.

- [ ] **Step 1: Add failing parser and response-header tests**

In `crates/remote-exec-daemon-cpp/tests/test_http_request.cpp`, after the existing `HTTP/2.0` rejection assertion, add:

```cpp
    assert_rejects(
        "POST /v1/exec/start HTTP/1.0\r\n"
        "\r\n"
    );
```

After the existing bearer-auth challenge assertions, add:

```cpp
    HttpResponse ok;
    write_json(ok, Json{{"status", "ok"}});
    const std::string rendered = render_http_response(ok);
    assert(rendered.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(rendered.find("Content-Type: application/json\r\n") != std::string::npos);
    assert(rendered.find("Content-Length: ") != std::string::npos);
    assert(rendered.find("Connection: close\r\n") == std::string::npos);
```

- [ ] **Step 2: Run the focused failing C++ test**

Run: `make -C crates/remote-exec-daemon-cpp test-host-http-request`

Expected: the test fails because `HTTP/1.0` is currently accepted and normal response rendering currently injects `Connection: close`.

- [ ] **Step 3: Checkpoint the failing-test diff**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/tests/test_http_request.cpp && git status --short`

Expected: no whitespace errors; `test_http_request.cpp` is listed as modified. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 4: C++ Parser and Header Implementation

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/http_request.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_helpers.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-http-request`

**Testing approach:** Existing failing tests from Task 3  
Reason: This task implements the minimal code to satisfy the parser and normal-response header tests before adding the larger persistent-socket behavior.

- [ ] **Step 1: Restrict the C++ parser to HTTP/1.1**

In `crates/remote-exec-daemon-cpp/src/http_request.cpp`, change:

```cpp
void validate_http_version(const std::string& value) {
    if (value != "HTTP/1.0" && value != "HTTP/1.1") {
        throw HttpParseError("unsupported http version");
    }
}
```

to:

```cpp
void validate_http_version(const std::string& value) {
    if (value != "HTTP/1.1") {
        throw HttpParseError("unsupported http version");
    }
}
```

- [ ] **Step 2: Remove normal response forced close**

In `crates/remote-exec-daemon-cpp/src/http_helpers.cpp`, change:

```cpp
    std::map<std::string, std::string> headers = res.headers;
    headers["Connection"] = "close";
    headers["Content-Length"] = std::to_string(res.body.size());
```

to:

```cpp
    std::map<std::string, std::string> headers = res.headers;
    headers["Content-Length"] = std::to_string(res.body.size());
```

- [ ] **Step 3: Remove streaming export forced close**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, change `send_transfer_export_headers` from:

```cpp
void send_transfer_export_headers(SOCKET client, const ExportedPayload& payload) {
    HttpResponse response;
    response.status = 200;
    response.headers["Connection"] = "close";
    response.headers["Transfer-Encoding"] = "chunked";
    write_transfer_export_headers(response, payload);
```

to:

```cpp
void send_transfer_export_headers(SOCKET client, const ExportedPayload& payload) {
    HttpResponse response;
    response.status = 200;
    response.headers["Transfer-Encoding"] = "chunked";
    write_transfer_export_headers(response, payload);
```

- [ ] **Step 4: Run focused C++ parser/header verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-http-request`

Expected: `test_host_http_request` passes.

- [ ] **Step 5: Checkpoint the task without committing**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/src/http_request.cpp crates/remote-exec-daemon-cpp/src/http_helpers.cpp crates/remote-exec-daemon-cpp/src/server.cpp crates/remote-exec-daemon-cpp/tests/test_http_request.cpp && git status --short`

Expected: no whitespace errors; the C++ parser/header files and test file are listed. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 5: C++ Persistent Socket Tests

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** TDD integration-style host test  
Reason: Persistent connection reuse is socket-level behavior. The existing socketpair test harness can prove two sequential requests are served on one accepted socket.

- [ ] **Step 1: Add response-reading helpers**

In `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`, add this include with the other standard headers:

```cpp
#include <cstdlib>
```

Add these helpers after `read_all_from_socket`:

```cpp
static void recv_exact_or_assert(SOCKET socket, char* data, std::size_t size) {
    std::size_t offset = 0;
    while (offset < size) {
        const int received = recv(
            socket,
            data + offset,
            static_cast<int>(size - offset),
            0
        );
        assert(received > 0);
        offset += static_cast<std::size_t>(received);
    }
}

static std::size_t response_content_length(const std::string& header_block) {
    const std::string marker = "\r\nContent-Length: ";
    const std::size_t start = header_block.find(marker);
    assert(start != std::string::npos);
    const std::size_t value_start = start + marker.size();
    const std::size_t value_end = header_block.find("\r\n", value_start);
    assert(value_end != std::string::npos);
    return static_cast<std::size_t>(
        std::strtoull(header_block.substr(value_start, value_end - value_start).c_str(), NULL, 10)
    );
}

static std::string read_content_length_response_from_socket(SOCKET socket) {
    std::string response;
    while (response.find("\r\n\r\n") == std::string::npos) {
        char ch = '\0';
        recv_exact_or_assert(socket, &ch, 1);
        response.push_back(ch);
    }

    const std::size_t header_end = response.find("\r\n\r\n");
    const std::size_t content_length = response_content_length(response.substr(0, header_end));
    const std::size_t total_size = header_end + 4U + content_length;
    while (response.size() < total_size) {
        char buffer[4096];
        const std::size_t remaining = total_size - response.size();
        const std::size_t request_size = std::min<std::size_t>(remaining, sizeof(buffer));
        recv_exact_or_assert(socket, buffer, request_size);
        response.append(buffer, request_size);
    }

    return response;
}
```

- [ ] **Step 2: Add a request helper with extra headers**

After the existing `json_post_request` helper, add:

```cpp
static std::string json_post_request_with_extra_headers(
    const std::string& path,
    const Json& body,
    const std::string& extra_headers
) {
    const std::string payload = body.dump();
    std::ostringstream request;
    request << "POST " << path << " HTTP/1.1\r\n"
            << "Content-Length: " << payload.size() << "\r\n"
            << extra_headers
            << "\r\n"
            << payload;
    return request.str();
}
```

- [ ] **Step 3: Add the sequential keepalive socket test helper**

After `json_post_request_with_extra_headers`, add:

```cpp
static void assert_persistent_json_requests_reuse_socket(AppState& state) {
    int sockets[2];
    assert(socketpair(AF_UNIX, SOCK_STREAM, 0, sockets) == 0);

    UniqueSocket server_socket(sockets[0]);
    UniqueSocket client_socket(sockets[1]);
    std::thread server_thread(
        [&state](SOCKET socket) {
            UniqueSocket owned_socket(socket);
            handle_client(state, std::move(owned_socket));
        },
        server_socket.release()
    );

    send_all(client_socket.get(), json_post_request("/v1/health", Json::object()));
    const std::string first_response =
        read_content_length_response_from_socket(client_socket.get());
    assert(first_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(first_response.find("Connection: close\r\n") == std::string::npos);
    assert(Json::parse(response_body(first_response)).at("status").get<std::string>() == "ok");

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
    assert(second_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(second_response.find("Connection: close\r\n") == std::string::npos);
    assert(Json::parse(response_body(second_response)).at("target").get<std::string>() == "cpp-test");

    char extra = '\0';
    assert(recv(client_socket.get(), &extra, 1, 0) == 0);
    server_thread.join();
}
```

- [ ] **Step 4: Assert streaming export does not advertise forced close**

In the existing export response assertions, add:

```cpp
    assert(export_response.find("Connection: close\r\n") == std::string::npos);
```

The block should include:

```cpp
    assert(export_response.find("HTTP/1.1 200 OK\r\n") == 0);
    assert(export_response.find("Transfer-Encoding: chunked\r\n") != std::string::npos);
    assert(export_response.find("Connection: close\r\n") == std::string::npos);
    assert(export_response.find("Content-Length:") == std::string::npos);
    assert(export_response.find("x-remote-exec-source-type: file\r\n") != std::string::npos);
```

- [ ] **Step 5: Call the persistent socket test**

In `main`, after `initialize_state(state, root);`, add:

```cpp
    assert_persistent_json_requests_reuse_socket(state);
```

- [ ] **Step 6: Run the focused failing C++ streaming test**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

Expected: compilation fails because `handle_client` is not declared yet, or runtime fails because the current handler processes only one request per socket.

- [ ] **Step 7: Checkpoint the failing-test diff**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp && git status --short`

Expected: no whitespace errors; `test_server_streaming.cpp` is listed as modified. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 6: C++ Persistent Socket Implementation

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/server_transport.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** Existing failing integration test from Task 5  
Reason: The implementation should be driven by the single-socket sequential request test and then exercised by all existing streaming, port-forward, and transfer cases in that test binary.

- [ ] **Step 1: Add clean-EOF request-head API declaration**

In `crates/remote-exec-daemon-cpp/include/server_transport.h`, add this declaration before `read_http_request_head`:

```cpp
bool try_read_http_request_head(
    SOCKET client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
);
```

The declarations should become:

```cpp
HttpRequestBodyFraming request_body_framing_from_headers(const std::string& header_block);
bool try_read_http_request_head(
    SOCKET client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
);
HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes);
std::string read_http_request(
    SOCKET client,
    std::size_t max_header_bytes,
    std::size_t max_body_bytes
);
```

- [ ] **Step 2: Implement clean EOF detection**

In `crates/remote-exec-daemon-cpp/src/server_transport.cpp`, replace `read_http_request_head` with this pair:

```cpp
bool try_read_http_request_head(
    SOCKET client,
    std::size_t max_header_bytes,
    HttpRequestHead* head
) {
    std::string data;
    char buffer[4096];

    for (;;) {
        const int received = recv(client, buffer, sizeof(buffer), 0);
        if (received == 0) {
            if (data.empty()) {
                return false;
            }
            break;
        }
        if (received < 0) {
            const int error = last_socket_error();
            if (would_block_error(error)) {
                continue;
            }
            throw std::runtime_error(socket_error_message("recv"));
        }

        data.append(buffer, received);
        const std::size_t header_end = data.find("\r\n\r\n");
        if (header_end == std::string::npos) {
            if (data.size() > max_header_bytes) {
                throw BadHttpRequest("http request headers too large");
            }
            continue;
        }

        if (header_end + 4U > max_header_bytes) {
            throw BadHttpRequest("http request headers too large");
        }

        head->raw_headers = data.substr(0, header_end);
        head->initial_body = data.substr(header_end + 4U);
        return true;
    }

    throw BadHttpRequest("incomplete http request");
}

HttpRequestHead read_http_request_head(SOCKET client, std::size_t max_header_bytes) {
    HttpRequestHead head;
    if (try_read_http_request_head(client, max_header_bytes, &head)) {
        return head;
    }

    throw BadHttpRequest("incomplete http request");
}
```

- [ ] **Step 3: Rename the public client handler**

In `crates/remote-exec-daemon-cpp/include/server.h`, change:

```cpp
void handle_client_once(AppState& state, UniqueSocket client);
```

to:

```cpp
void handle_client(AppState& state, UniqueSocket client);
```

In `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`, change the two existing helper calls from:

```cpp
    handle_client_once(state, std::move(server_socket));
```

to:

```cpp
    handle_client(state, std::move(server_socket));
```

- [ ] **Step 4: Add C++ connection-close token detection**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, add this include:

```cpp
#include "text_utils.h"
```

In the anonymous namespace, before `bool reject_before_route`, add:

```cpp
bool request_connection_close_requested(const HttpRequest& request) {
    const std::string value = lowercase_ascii(request.header("connection"));
    std::size_t offset = 0;
    while (offset <= value.size()) {
        const std::size_t comma = value.find(',', offset);
        const std::string token = trim_ascii(
            comma == std::string::npos
                ? value.substr(offset)
                : value.substr(offset, comma - offset)
        );
        if (token == "close") {
            return true;
        }
        if (comma == std::string::npos) {
            return false;
        }
        offset = comma + 1U;
    }

    return false;
}
```

- [ ] **Step 5: Refactor one request into a reusable helper**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, replace the current `void handle_client_once(AppState& state, UniqueSocket client)` function with this helper plus persistent handler:

```cpp
int handle_client_request(
    AppState& state,
    SOCKET client,
    const HttpRequestHead& request_head,
    bool* close_after_response
) {
    const std::uint64_t started_at_ms = platform::monotonic_ms();
    HttpRequest request = parse_http_request_head(request_head.raw_headers);
    *close_after_response = request_connection_close_requested(request);
    const HttpRequestBodyFraming framing =
        request_body_framing_from_headers(request_head.raw_headers);
    HttpRequestBodyStream body(
        client,
        request_head.initial_body,
        framing,
        state.config.max_request_body_bytes
    );

    if (request.path == "/v1/transfer/export") {
        const int status = handle_streaming_transfer_export(state, request, &body, client);
        log_request_result(request, status, started_at_ms);
        return status;
    }

    HttpResponse response;
    if (request.path == "/v1/transfer/import") {
        response = handle_streaming_transfer_import(state, request, &body);
    } else {
        request.body = read_request_body_to_string(&body);
        response = route_request(state, request);
    }
    log_request_result(request, response.status, started_at_ms);
    if (!try_send_response(client, response)) {
        *close_after_response = true;
    }
    return response.status;
}

void handle_client(AppState& state, UniqueSocket client) {
    for (;;) {
        try {
            HttpRequestHead request_head;
            if (!try_read_http_request_head(
                    client.get(),
                    state.config.max_request_header_bytes,
                    &request_head
                )) {
                return;
            }

            bool close_after_response = false;
            handle_client_request(state, client.get(), request_head, &close_after_response);
            if (close_after_response) {
                return;
            }
        } catch (const BadHttpRequest& ex) {
            log_message(LOG_WARN, "server", ex.what());
            HttpResponse response;
            response.status = 400;
            write_rpc_error(response, 400, "bad_request", ex.what());
            try_send_response(client.get(), response);
            return;
        } catch (const HttpParseError& ex) {
            log_message(LOG_WARN, "server", ex.what());
            HttpResponse response;
            response.status = 400;
            write_rpc_error(response, 400, "bad_request", ex.what());
            try_send_response(client.get(), response);
            return;
        } catch (const SocketSendError& ex) {
            log_send_failure(ex);
            return;
        } catch (const std::exception& ex) {
            log_message(LOG_ERROR, "server", ex.what());
            HttpResponse response;
            response.status = 500;
            write_rpc_error(response, 500, "internal_error", ex.what());
            try_send_response(client.get(), response);
            return;
        }
    }
}
```

- [ ] **Step 6: Make send failure close the keepalive loop**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, change `try_send_response` from:

```cpp
bool try_send_response(SOCKET client, const HttpResponse& response) {
    try {
        send_all(client, render_http_response(response));
        return true;
    } catch (const SocketSendError& ex) {
        return log_send_failure(ex);
    }
}
```

to:

```cpp
bool try_send_response(SOCKET client, const HttpResponse& response) {
    try {
        send_all(client, render_http_response(response));
        return true;
    } catch (const SocketSendError& ex) {
        log_send_failure(ex);
        return false;
    }
}
```

- [ ] **Step 7: Update client thread entry points**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, change both remaining references to the old handler name:

```cpp
    handle_client_once(*context->state, std::move(client));
```

and:

```cpp
            handle_client_once(state, std::move(thread_client));
```

to:

```cpp
    handle_client(*context->state, std::move(client));
```

and:

```cpp
            handle_client(state, std::move(thread_client));
```

- [ ] **Step 8: Run focused C++ persistent verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

Expected: `test_host_server_streaming` passes, including existing transfer, sandbox, port-forward, abort-client, and new persistent sequential request coverage.

- [ ] **Step 9: Run adjacent C++ transport verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`

Expected: `test_host_server_transport` passes, proving existing request body framing helpers still work.

- [ ] **Step 10: Checkpoint the task without committing**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/include/server_transport.h crates/remote-exec-daemon-cpp/src/server_transport.cpp crates/remote-exec-daemon-cpp/include/server.h crates/remote-exec-daemon-cpp/src/server.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp && git status --short`

Expected: no whitespace errors; the C++ server, transport, header, and streaming test files are listed. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 7: Documentation Updates

**Files:**
- Modify: `README.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Test/Verify: `rg -n "HTTP/1.1|keepalive|pipelining" README.md crates/remote-exec-daemon-cpp/README.md`

**Testing approach:** Documentation-only targeted verification  
Reason: The transport behavior is internal but operator-visible for daemon compatibility, so README updates are enough and no generated docs are involved.

- [ ] **Step 1: Update the main README architecture section**

In `README.md`, after the architecture bullet:

```markdown
- The broker validates `target`, forwards the request to the selected daemon, and returns MCP-compatible content plus structured JSON for tools that expose it unless `disable_structured_content = true` is configured.
```

add:

```markdown
- Broker-daemon RPC uses HTTP/1.1 only. The broker keeps daemon connections eligible for client-side pooling instead of forcing every request to close the TCP connection.
```

- [ ] **Step 2: Update the C++ daemon limitations**

In `crates/remote-exec-daemon-cpp/README.md`, under `## Limitations`, after:

```markdown
- plain HTTP only, with optional bearer-auth request authentication
```

add:

```markdown
- daemon RPC is HTTP/1.1-only; sequential requests may reuse a persistent connection, but HTTP pipelining is not supported
```

- [ ] **Step 3: Verify docs mention the new transport contract**

Run: `rg -n "HTTP/1.1|pipelining|pooling" README.md crates/remote-exec-daemon-cpp/README.md`

Expected: output includes the new README bullet and the new C++ daemon limitations bullet.

- [ ] **Step 4: Checkpoint the docs without committing**

Run: `git diff --check -- README.md crates/remote-exec-daemon-cpp/README.md && git status --short`

Expected: no whitespace errors; both README files are listed. Do not run `git commit` unless the user explicitly requests commits.

---

### Task 8: Focused and Broader Verification

**Files:**
- Test/Verify: Rust broker, Rust daemon, C++ daemon focused suites

**Testing approach:** Existing tests + targeted verification  
Reason: Behavior spans the broker client, Rust daemon HTTP layer, and C++ daemon socket server. Focused tests should run first; broader checks should run after focused confidence.

- [ ] **Step 1: Run broker focused tests**

Run: `cargo test -p remote-exec-broker daemon_request --lib`

Expected: broker daemon request header tests pass.

- [ ] **Step 2: Run Rust daemon focused tests**

Run: `cargo test -p remote-exec-daemon --test health`

Expected: health tests pass, including `daemon_rejects_http_1_0_rpc_requests`.

- [ ] **Step 3: Run C++ focused tests**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-http-request
make -C crates/remote-exec-daemon-cpp test-host-server-transport
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: all three C++ host tests pass.

- [ ] **Step 4: Run C++ POSIX aggregate check**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`

Expected: all POSIX C++ host tests and POSIX daemon build pass.

- [ ] **Step 5: Run Rust formatting check**

Run: `cargo fmt --all --check`

Expected: formatting check passes.

- [ ] **Step 6: Run Rust workspace tests**

Run: `cargo test --workspace`

Expected: workspace tests pass.

- [ ] **Step 7: Run Rust clippy quality gate**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

Expected: clippy completes without warnings.

- [ ] **Step 8: Final diff review**

Run:

```bash
git diff --check
git status --short
git diff --stat
```

Expected: no whitespace errors; only intended source, test, README, spec, and plan files are listed; diff stat matches the implementation scope. Do not run `git commit` unless the user explicitly requests commits.
