# C++ HTTP Server Internal Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Refactor the C++ daemon HTTP server internals to remove duplicated parsing and transfer logic, delete dead test-only production code, and move per-connection request handling out of `server.cpp` without changing observable daemon behavior.

**Architecture:** Introduce a shared HTTP codec module for body framing and chunk parsing, a shared server-request utility layer for auth/method/sandbox/path handling, and a dedicated connection-serving implementation file for sequential request processing. Keep all public endpoints, error codes, and current persistent-connection behavior unchanged while preserving the existing POSIX host test coverage.

**Tech Stack:** C++11 production target, C++17 host tests, POSIX socketpair-based HTTP tests, existing Makefile targets under `crates/remote-exec-daemon-cpp`

---

## File Structure

- `crates/remote-exec-daemon-cpp/include/http_codec.h`
  - New shared declarations for HTTP request body framing and chunk/content-length parsing.

- `crates/remote-exec-daemon-cpp/src/http_codec.cpp`
  - New single implementation of content-length parsing, chunk-size parsing, chunked body decoding, and header-based request-body framing.

- `crates/remote-exec-daemon-cpp/include/server_request_utils.h`
  - New shared declarations for request preflight, sandbox helpers, workdir/path resolution, and transfer request preparation.

- `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
  - New shared implementation used by route handlers and streaming transfer handlers.

- `crates/remote-exec-daemon-cpp/src/http_request.cpp`
  - Switch to the shared HTTP codec instead of local chunk/content-length parsers.

- `crates/remote-exec-daemon-cpp/include/server_transport.h`
  - Remove the dead `read_http_request` declaration and include the shared framing type through the new codec header.

- `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
  - Use the shared HTTP codec for body framing and chunk parsing, keep only socket I/O and body-stream mechanics.

- `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
  - Replace duplicated transfer parsing/sandbox/error code with shared utility functions.

- `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
  - Reuse shared preflight validation instead of duplicating auth/method checks.

- `crates/remote-exec-daemon-cpp/include/server.h`
  - Keep `handle_client` and `run_server` declarations stable for existing tests.

- `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
  - New dedicated home for `handle_client`, request-loop orchestration, streaming transfer send path, and connection-level error handling.

- `crates/remote-exec-daemon-cpp/src/server.cpp`
  - Shrink to daemon bootstrapping, listener accept loop, and thread spawning only.

- `crates/remote-exec-daemon-cpp/Makefile`
  - Add the new compilation units to production and relevant host-test targets.

- `crates/remote-exec-daemon-cpp/tests/test_http_request.cpp`
  - Keep parser coverage pointed at public request parsing while the codec moves underneath it.

- `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`
  - Stop relying on the dead whole-request production helper and validate `try_read_http_request_head` plus `HttpRequestBodyStream` directly.

- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
  - Preserve sequential keepalive and streaming transfer behavior through the new connection-serving file.

---

### Task 1: Replace the dead whole-request test seam

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server_transport.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`

**Testing approach:** `characterization/integration test`
Reason: this is a structural cleanup with an existing test seam, so the right move is to preserve the current decoded-body behavior while removing the dead production-only helper.

- [ ] **Step 1: Rewrite the transport test around the real production streaming API**

```cpp
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
```

- [ ] **Step 2: Run the focused transport test before deleting the helper**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
Expected: PASS while `read_http_request` still exists, proving the new test matches current behavior.

- [ ] **Step 3: Delete the dead production helper declaration and implementation**

```cpp
// Remove from include/server_transport.h:
std::string read_http_request(
    SOCKET client,
    std::size_t max_header_bytes,
    std::size_t max_body_bytes
);

// Remove the matching definition from src/server_transport.cpp.
```

- [ ] **Step 4: Re-run the focused transport verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
Expected: PASS with no references to `read_http_request` remaining.

- [ ] **Step 5: Checkpoint the slice without committing**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/include/server_transport.h crates/remote-exec-daemon-cpp/src/server_transport.cpp crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp && git status --short`
Expected: no whitespace errors; only the expected transport/test files are modified.

---

### Task 2: Extract the shared HTTP codec

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/http_codec.h`
- Create: `crates/remote-exec-daemon-cpp/src/http_codec.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_request.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server_transport.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-http-request test-host-server-transport`

**Testing approach:** `existing tests + targeted verification`
Reason: the observable parser behavior is already covered, and this task is an internal extraction rather than a behavior change.

- [ ] **Step 1: Add the shared codec interface**

```cpp
class HttpProtocolError : public std::runtime_error {
public:
    explicit HttpProtocolError(const std::string& message)
        : std::runtime_error(message) {}
};

struct HttpRequestBodyFraming {
    HttpRequestBodyFraming();
    bool has_content_length;
    std::size_t content_length;
    bool chunked;
};

HttpRequestBodyFraming request_body_framing_from_headers(
    const std::map<std::string, std::string>& headers
);
std::size_t parse_http_chunk_size_line(const std::string& line);
std::string decode_http_chunked_body(const std::string& body);
```

- [ ] **Step 2: Move duplicate chunk/content-length logic into `http_codec.cpp` and adapt callers**

```cpp
// In http_request.cpp:
const HttpRequestBodyFraming framing =
    request_body_framing_from_headers(request.headers);
if (framing.chunked) {
    request.body = decode_http_chunked_body(raw_body);
} else {
    if (framing.has_content_length && raw_body.size() != framing.content_length) {
        throw HttpParseError("Content-Length does not match body size");
    }
    request.body = raw_body;
}

// In server_transport.cpp:
const std::size_t chunk_size =
    parse_http_chunk_size_line(raw_.substr(raw_offset_, line_end - raw_offset_));
```

- [ ] **Step 3: Make transport headers use the parsed request headers instead of reparsing raw text**

```cpp
HttpRequest request = parse_http_request_head(request_head.raw_headers);
const HttpRequestBodyFraming framing =
    request_body_framing_from_headers(request.headers);
```

- [ ] **Step 4: Run the focused parser and transport verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-http-request test-host-server-transport`
Expected: PASS with the new codec compiled into both test targets.

- [ ] **Step 5: Checkpoint the slice without committing**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/include/http_codec.h crates/remote-exec-daemon-cpp/src/http_codec.cpp crates/remote-exec-daemon-cpp/src/http_request.cpp crates/remote-exec-daemon-cpp/include/server_transport.h crates/remote-exec-daemon-cpp/src/server_transport.cpp crates/remote-exec-daemon-cpp/Makefile && git status --short`
Expected: no whitespace errors; the codec, transport, parser, and Makefile edits are present.

---

### Task 3: Collapse duplicated request and transfer preparation

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/server_request_utils.h`
- Create: `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_exec.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_image.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `existing tests + targeted verification`
Reason: the existing route suite already locks the public daemon behavior, and this task is primarily deduplicating path/sandbox/transfer preflight code.

- [ ] **Step 1: Introduce shared request utility declarations**

```cpp
bool reject_before_route(
    const AppState& state,
    const HttpRequest& request,
    HttpResponse* response
);

std::string resolve_workdir(const AppState& state, const Json& body);
std::string resolve_input_path(
    const AppState& state,
    const Json& body,
    const std::string& key
);
void authorize_sandbox_path(
    const AppState& state,
    SandboxAccess access,
    const std::string& path
);
std::string resolve_absolute_transfer_path(const std::string& path);
std::vector<std::string> transfer_exclude_or_empty(const Json& body);
```

- [ ] **Step 2: Move shared auth/method/path helpers out of route and server files**

```cpp
// Replace local anonymous-namespace copies with calls to
// resolve_workdir(...)
// resolve_input_path(...)
// authorize_sandbox_path(...)
// resolve_absolute_transfer_path(...)
// reject_before_route(...)
```

- [ ] **Step 3: Make route dispatch reuse the shared preflight**

```cpp
HttpResponse response;
response.status = 200;
if (reject_before_route(state, request, &response)) {
    return response;
}
```

- [ ] **Step 4: Run the focused route verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS with unchanged route-level behavior.

- [ ] **Step 5: Checkpoint the slice without committing**

Run: `git diff --check -- crates/remote-exec-daemon-cpp/include/server_request_utils.h crates/remote-exec-daemon-cpp/src/server_request_utils.cpp crates/remote-exec-daemon-cpp/src/server_route_common.cpp crates/remote-exec-daemon-cpp/src/server_route_exec.cpp crates/remote-exec-daemon-cpp/src/server_route_image.cpp crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp crates/remote-exec-daemon-cpp/src/server_routes.cpp crates/remote-exec-daemon-cpp/Makefile && git status --short`
Expected: no whitespace errors; request utility and route files are modified.

---

### Task 4: Split per-connection request serving out of `server.cpp`

**Files:**
- Create: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming test-host-server-routes test-host-server-transport`

**Testing approach:** `existing tests + targeted verification`
Reason: this is a file-boundary refactor of the current connection loop, and the existing streaming/route/transport suites already cover the external behavior that must stay stable.

- [ ] **Step 1: Move the per-socket request loop into a dedicated file**

```cpp
// src/http_connection.cpp
int handle_client_request(
    AppState& state,
    SOCKET client,
    const HttpRequestHead& request_head,
    bool* close_after_response
);

void handle_client(AppState& state, UniqueSocket client) {
    for (;;) {
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
    }
}
```

- [ ] **Step 2: Keep `server.cpp` focused on process bootstrap and thread spawning**

```cpp
// Leave in server.cpp:
std::string daemon_instance_id();
void spawn_client_thread(AppState& state, UniqueSocket client);
int run_server(const DaemonConfig& config);

// Remove from server.cpp:
// - streaming transfer helpers
// - try_send_response/log_send_failure
// - handle_client_request
// - handle_client loop
```

- [ ] **Step 3: Compile the new connection file into production and host tests**

```make
BASE_SRCS := ... src/server.cpp src/http_connection.cpp src/server_transport.cpp ...
HOST_SERVER_STREAMING_SRCS := ... src/server.cpp src/http_connection.cpp ...
HOST_SERVER_ROUTES_SRCS := ... src/server_transport.cpp src/http_connection.cpp ...
```

- [ ] **Step 4: Run the connection-oriented verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming test-host-server-routes test-host-server-transport`
Expected: PASS with persistent sequential requests and transfer streaming unchanged.

- [ ] **Step 5: Run the C++ daemon quality gate for the touched area**

Run: `make -C crates/remote-exec-daemon-cpp test-host-http-request test-host-server-transport test-host-server-streaming test-host-server-routes check-posix`
Expected: PASS.

- [ ] **Step 6: Checkpoint the finished refactor without committing**

```bash
git diff --check
git status --short
```

Expected: no whitespace errors; only the planned daemon C++ files and this plan are modified. Do not run `git commit` unless the user explicitly requests commits.
