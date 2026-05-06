# Daemon HTTP Keepalive Design

Status: approved design captured in writing

Date: 2026-05-06

References:

- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-daemon/src/tls.rs`
- `crates/remote-exec-daemon/src/tls_enabled.rs`
- `crates/remote-exec-daemon-cpp/src/http_helpers.cpp`
- `crates/remote-exec-daemon-cpp/src/http_request.cpp`
- `crates/remote-exec-daemon-cpp/src/server.cpp`
- `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- `crates/remote-exec-daemon-cpp/include/http_helpers.h`
- `crates/remote-exec-daemon-cpp/include/server_transport.h`
- `crates/remote-exec-daemon-cpp/tests/test_http_request.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`

## Goal

Enable true persistent HTTP/1.1 connection reuse for broker-to-daemon RPCs.

The broker should stop forcing every daemon request to close the TCP connection.
The Rust daemon should continue using its existing HTTP/1.1 server behavior. The
C++ daemon should stop advertising forced close and should serve multiple
sequential HTTP/1.1 requests on the same accepted socket.

Both daemon implementations should also drop HTTP/1.0 support because the broker
and daemon are the only intended users of this internal transport.

## Scope

This design covers:

- broker daemon-client request headers
- Rust daemon HTTP/1.1-only validation
- C++ daemon HTTP response connection headers
- C++ daemon per-socket request loop
- daemon request parsing/validation rules for HTTP version and `Connection`
- normal JSON RPC responses
- streaming transfer export responses
- streaming transfer import requests
- tests that prove connection reuse across sequential RPCs

This design does not cover:

- adding public configuration for keepalive behavior
- adding HTTP/2 support
- adding HTTP pipelining support
- changing public MCP tool schemas
- changing broker target configuration shape
- changing daemon authentication, TLS, sandbox, or transfer semantics
- adding keepalive support to any third-party HTTP library path

## Problem Statement

The broker currently sends `Connection: close` for every daemon RPC from
`DaemonClient::request`. That prevents `reqwest::Client` from reusing pooled
daemon connections even though most daemon RPCs are ordinary HTTP/1.1 requests.

The Rust daemon uses Hyper HTTP/1.1 serving for both plain HTTP and TLS daemon
transports. Those paths already support persistent HTTP/1.1 connections unless
the client asks to close or the daemon is shutting down.

The C++ daemon is different. It explicitly writes `Connection: close` in normal
HTTP responses and in streaming transfer export headers. It also handles exactly
one request per accepted socket before the `UniqueSocket` closes the connection.
Removing the close headers alone would therefore be incomplete: the broker could
attempt to reuse a pooled connection that the C++ daemon has already closed.

True persistent broker-daemon reuse requires changing both sides:

- the broker must stop forcing close
- the C++ daemon must serve more than one sequential request per connection

## Protocol Decisions

### HTTP Version

Broker-daemon communication is HTTP/1.1 only.

Both daemon implementations should reject daemon RPC requests whose HTTP version
is not `HTTP/1.1`. This intentionally drops the current C++ parser acceptance of
`HTTP/1.0` and avoids keeping compatibility branches for clients that should not
exist.

The broker uses `reqwest`, which sends HTTP/1.1 for these daemon RPCs.

### Connection Header

The broker should not set the `Connection` request header for daemon RPCs.

The C++ daemon should not set `Connection: close` on successful normal responses,
error responses, authentication challenges, or streaming export responses.

The C++ daemon should still respect a client request containing
`Connection: close` by closing the socket after sending the current response.
This keeps behavior compatible with standard HTTP/1.1 clients and with manual
diagnostic tools.

Unsupported `Connection` token values do not need special validation. The daemon
can ignore values other than `close` because the transport is internal and
HTTP/1.1 persistence is the default.

### Request Boundaries

The C++ daemon should continue supporting request bodies framed by either:

- `Content-Length`
- `Transfer-Encoding: chunked`

Every response must remain self-delimiting:

- normal responses use `Content-Length`
- transfer export responses use `Transfer-Encoding: chunked` plus the terminating
  zero-length chunk

### Pipelining

The C++ daemon should support sequential keepalive reuse, not HTTP pipelining.

The broker's reqwest pool does not require pipelining. The implementation should
not advertise pipelining support and should not add buffering complexity solely
to preserve bytes for a second request that was sent before the first response
completed.

## Rust Broker Design

`crates/remote-exec-broker/src/daemon_client.rs` should remove the import and use
of `reqwest::header::CONNECTION`.

`DaemonClient::request` should build the request with the daemon URL and optional
authorization header only. All call sites that use `request` then inherit normal
`reqwest::Client` connection pooling:

- JSON RPCs via `post`
- transfer export
- transfer import from file
- transfer import from streaming body
- port-forward lease and connection RPCs

No public API or target configuration changes are needed.

## Rust Daemon Design

The plain HTTP path in `crates/remote-exec-daemon/src/tls.rs` and the TLS path in
`crates/remote-exec-daemon/src/tls_enabled.rs` already use Hyper
`http1::Builder::serve_connection`. Those paths should remain structurally
unchanged unless tests expose an issue.

The Rust daemon router should add focused HTTP-version validation for daemon RPC
requests. A small middleware in the existing HTTP layer can inspect
`Request::version()` and return the existing JSON RPC bad-request style response
when the version is not `HTTP/1.1`.

The middleware should run before route handlers. It can be placed near the
existing auth and request-log middleware without changing the daemon public route
shape.

Shutdown behavior should stay the same:

- daemon shutdown sends the connection-shutdown watch signal
- active Hyper connections receive `graceful_shutdown`
- connection tasks are joined before the listener reports stopped

## C++ Daemon Design

### HTTP Parser

`crates/remote-exec-daemon-cpp/src/http_request.cpp` should reject request lines
unless the version is exactly `HTTP/1.1`.

The error can reuse the existing `"unsupported http version"` message unless
tests need a more precise message. Existing tests that assert `HTTP/2.0` is
rejected should continue passing. Add coverage that `HTTP/1.0` is rejected.

### Normal Response Rendering

`render_http_response` in `crates/remote-exec-daemon-cpp/src/http_helpers.cpp`
should stop injecting `Connection: close`.

It should continue copying response-specific headers and setting
`Content-Length` to the response body byte length.

No route handler should need to set a connection header for normal operation.

### Streaming Transfer Export

`send_transfer_export_headers` in `crates/remote-exec-daemon-cpp/src/server.cpp`
should stop setting `Connection: close`.

It should continue setting `Transfer-Encoding: chunked`, applying transfer
metadata headers, sending the blank header terminator, streaming archive chunks,
and sending the terminating `0\r\n\r\n` chunk on success.

If streaming export fails after headers have been sent, the current behavior is
already limited because the status and headers are committed. This change should
not broaden that scope.

### Per-Socket Request Loop

The C++ daemon should replace the one-request socket flow with a loop such as:

1. read one request head from the socket
2. parse the request
3. build a body stream for that request
4. route the request and send the response
5. decide whether to continue or close

The loop should continue after successful HTTP/1.1 requests unless the request
has `Connection: close` or the server encounters a read, parse, route, or send
error.

The loop should close after sending an error response for malformed requests.
That keeps recovery simple and avoids trying to resynchronize a potentially
corrupt HTTP byte stream.

The existing exported function name can stay `handle_client_once` if broad test
renaming is not worth it, but a clearer implementation name such as
`handle_client` is preferable if the change remains focused.

### Request Body Consumption

Before continuing to the next request, each handler must fully consume the
current request body according to its framing:

- normal JSON RPCs already read the body into `request.body`
- streaming transfer import reads the archive body while importing
- streaming transfer export reads the JSON request body before exporting

If a handler returns before consuming the body, the connection must close rather
than attempting to parse the next request from a partially consumed stream.

The initial implementation can rely on current handlers consuming their bodies,
with tests covering the known request types used for persistent reuse.

### Connection Close Detection

Add a small helper that detects `Connection: close` in the parsed request
headers. The C++ parser stores lower-case header names, so the helper should
read `request.header("connection")`.

The value should be treated case-insensitively and should handle comma-separated
tokens so values like `keep-alive, close` still close.

No `Connection: keep-alive` support is needed because HTTP/1.1 persistence is
the default and HTTP/1.0 is no longer accepted.

## Testing Design

### Broker Tests

Add or adjust Rust broker coverage so `DaemonClient` no longer emits
`Connection: close`.

Preferred test shape:

- use a lightweight local HTTP test server or existing daemon fixture
- make at least two daemon RPC calls through one `DaemonClient`
- assert the server observes both requests on the same accepted TCP connection
- assert no request contains `Connection: close`

If a direct broker-unit seam is too expensive, the C++ integration test below is
the primary proof and a smaller unit test can focus on request header absence.

### Rust Daemon Tests

Add focused daemon coverage for HTTP/1.1-only behavior and, if cheap, persistent
reuse across two sequential requests. The existing Hyper server behavior makes
the reuse portion lower risk than the C++ daemon path.

Focused Rust daemon tests should:

- reject a raw `HTTP/1.0` daemon RPC request with a bad-request JSON response
- accept a normal `HTTP/1.1` daemon RPC request

If adding reuse coverage, the test should:

- open one TCP connection to the daemon
- send two sequential HTTP/1.1 POST requests
- read two successful responses
- avoid depending on timing-sensitive connection-pool internals

### C++ Daemon Tests

Add C++ host tests for:

- `HTTP/1.0` request parsing is rejected
- `render_http_response` no longer emits `Connection: close`
- a single socket can carry two sequential HTTP/1.1 requests and receive two
  valid responses
- a request with `Connection: close` receives its response and then closes the
  socket
- streaming transfer export response headers no longer emit `Connection: close`

The existing `test_server_streaming` socket-based style is the best place for
end-to-end C++ daemon socket behavior.

## Compatibility

This changes only the internal broker-daemon HTTP behavior.

The public MCP tool surface remains unchanged. Target selection, authentication,
sandbox checks, transfer metadata, transfer archive formats, port-forward state,
and broker-owned public session IDs remain unchanged.

Dropping daemon HTTP/1.0 support is acceptable because the transport is intended
for broker-daemon communication, and the broker uses HTTP/1.1-capable client
infrastructure.

## Risks

### Over-Read and Pipelining

The C++ streaming body reader may read bytes beyond the current request body if a
client pipelines another request before the current response completes. This
design explicitly does not support pipelining, so that case can close or fail
rather than requiring a buffered carry-over mechanism.

### Idle Connections

Persistent connections can leave C++ daemon client threads blocked waiting for a
future request. This is acceptable for broker-managed pooled connections, but it
does mean dead clients are cleaned up by TCP close/reset rather than an
application-level idle timeout.

If this becomes a practical issue later, add a small socket read timeout as a
separate change. Do not add that complexity to the initial keepalive change.

### Long-Lived Streaming Responses

Transfer export holds the connection for the duration of the streamed archive.
That is already true today. After this change, the same connection may be reused
after the terminating chunk is sent, which relies on correct chunk framing.

### Error Recovery

After malformed requests or partial bodies, the C++ daemon should close the
socket after sending an error response. Attempting to recover and reuse the same
connection would add unnecessary parser state complexity.

## Validation

Focused validation should run before broader checks:

- `make -C crates/remote-exec-daemon-cpp test-host-http-request`
- `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
- `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- targeted broker/daemon Rust tests added or changed by the implementation

If public-surface or cross-cutting behavior changes unexpectedly, finish with:

- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `make -C crates/remote-exec-daemon-cpp check-posix`
