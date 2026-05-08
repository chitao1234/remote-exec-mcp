# Port Forward V4 Parity And Limits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Make the standalone C++ daemon truthfully implement and advertise v4 port tunnel limits, then enforce the Rust broker and Rust daemon/host limits and correct broker forwarding state semantics.

**Architecture:** The broker keeps public forward/session identity and cross-side state, while each host implementation owns daemon-local tunnel resources. C++ gets first-class work: config shape, advertised v4 limits, retained session/listener/bind/stream enforcement, blocking upgraded-socket liveness, tests, and docs. Rust then consumes existing config rather than fixed constants, enforces reconnection and stream accounting at the broker, and enforces host-side tunnel budgets in the shared host runtime.

**Tech Stack:** Rust 2024, Tokio, Axum/hyper upgrade streams, `remote-exec-proto` v4 tunnel frames, C++11-compatible daemon code with POSIX/Win32 socket helpers, Make test targets, Cargo workspace tests.

---

### Task 0: Commit This Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-08-port-forward-v4-parity-and-limits.md`
- Test/Verify: `git diff -- docs/superpowers/plans/2026-05-08-port-forward-v4-parity-and-limits.md`

**Testing approach:** no new tests needed
Reason: This is a planning artifact only; correctness is checked by reviewing the saved plan and committing it before behavior changes.

- [ ] **Step 1: Review the plan diff**

Run: `git diff -- docs/superpowers/plans/2026-05-08-port-forward-v4-parity-and-limits.md`
Expected: The new plan describes C++ parity work before Rust work and includes per-task commit steps.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/plans/2026-05-08-port-forward-v4-parity-and-limits.md
git commit -m "plan: outline port forward v4 parity work"
```

### Task 1: C++ Configurable V4 Tunnel Limit Advertisement

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/port_tunnel.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-config`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** TDD
Reason: The observable contract is concrete: config parsing must reject invalid limits, and `TunnelReady.limits` must report the configured values rather than hard-coded defaults.

- [ ] **Step 1: Write failing config tests**

Add C++ assertions in `tests/test_config.cpp` for these config keys:

```ini
port_forward_max_retained_sessions = 11
port_forward_max_retained_listeners = 13
port_forward_max_udp_binds = 15
port_forward_max_active_tcp_streams = 19
port_forward_max_tunnel_queued_bytes = 2097152
port_forward_tunnel_io_timeout_ms = 7000
```

Add a rejection check for each key set to `0`, with assertion helper code equivalent to:

```cpp
static bool config_rejected(const fs::path& path) {
    try {
        (void)load_config(path.string());
    } catch (...) {
        return true;
    }
    return false;
}
```

- [ ] **Step 2: Write failing advertised-limit test**

In `tests/test_server_streaming.cpp`, add an initializer that sets all C++ port tunnel limits to non-default values, opens a v4 tunnel, sends `TunnelOpen`, reads `TunnelReady`, and asserts:

```cpp
const Json limits = ready_meta.at("limits");
assert(limits.at("max_active_tcp_streams").get<unsigned long>() == 3UL);
assert(limits.at("max_udp_peers").get<unsigned long>() == 5UL);
assert(limits.at("max_queued_bytes").get<unsigned long>() == 4096UL);
```

- [ ] **Step 3: Run the red checks**

Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
Expected: FAIL because the new `DaemonConfig` fields do not exist or are not parsed.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: FAIL because `TunnelReady.limits` still reports hard-coded values.

- [ ] **Step 4: Implement config and advertised limits**

Add a C++11-compatible config struct and defaults in `include/config.h`:

```cpp
struct PortForwardLimitConfig {
    unsigned long max_worker_threads;
    unsigned long max_retained_sessions;
    unsigned long max_retained_listeners;
    unsigned long max_udp_binds;
    unsigned long max_active_tcp_streams;
    unsigned long max_tunnel_queued_bytes;
    unsigned long tunnel_io_timeout_ms;
};
```

Keep `port_forward_max_worker_threads` as a top-level compatibility field and add `port_forward_limits` to `DaemonConfig`. Parse the new keys in `src/config.cpp`, validate every value is greater than zero, and set `config.port_forward_max_worker_threads = config.port_forward_limits.max_worker_threads`.

Change `create_port_tunnel_service` to accept a `PortForwardLimitConfig` and update server initialization paths to pass `config.port_forward_limits`. Store the config in `PortTunnelService`, expose `limits() const`, and build `TunnelReady.limits` from the stored values:

```json
{
  "max_active_tcp_streams": service_->limits().max_active_tcp_streams,
  "max_udp_peers": service_->limits().max_udp_binds,
  "max_queued_bytes": service_->limits().max_tunnel_queued_bytes
}
```

- [ ] **Step 5: Run post-change verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/config.h \
  crates/remote-exec-daemon-cpp/src/config.cpp \
  crates/remote-exec-daemon-cpp/include/port_tunnel.h \
  crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h \
  crates/remote-exec-daemon-cpp/src/port_tunnel.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp \
  crates/remote-exec-daemon-cpp/src/server_runtime.cpp \
  crates/remote-exec-daemon-cpp/src/http_connection.cpp \
  crates/remote-exec-daemon-cpp/tests/test_config.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "feat: add cpp port tunnel limit config"
```

### Task 2: C++ Retained Resource And Active Stream Enforcement

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** TDD
Reason: Each resource budget has a direct tunnel-frame behavior seam: the daemon must emit `Error` frames with `port_tunnel_limit_exceeded` and release counters when resources close or expire.

- [ ] **Step 1: Write failing retained/session limit tests**

In `tests/test_server_streaming.cpp`, add tests that use tiny configured limits:

```cpp
// max_retained_sessions = 1
// Open one retained SessionOpen successfully.
// Open a second retained SessionOpen on another upgraded tunnel.
// Expect Error code "port_tunnel_limit_exceeded".
```

```cpp
// max_retained_listeners = 1
// Open SessionOpen, then TcpListen stream 1 successfully.
// Send TcpListen stream 3.
// Expect Error on stream 3 with code "port_tunnel_limit_exceeded".
// Close the first listener, then verify a new listener can open.
```

- [ ] **Step 2: Write failing UDP and active TCP stream tests**

Add tests:

```cpp
// max_udp_binds = 1
// First UdpBind succeeds, second UdpBind returns Error code "port_tunnel_limit_exceeded".
// Closing the first bind releases the budget and a later bind succeeds.
```

```cpp
// max_active_tcp_streams = 1
// One TcpConnect succeeds and remains open.
// A second TcpConnect returns Error code "port_tunnel_limit_exceeded".
// Closing the first stream releases the budget and a later TcpConnect succeeds.
```

- [ ] **Step 3: Run red check**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: FAIL because limits are advertised but not enforced.

- [ ] **Step 4: Implement counters and release guards**

Add atomic counters to `PortTunnelService`:

```cpp
std::atomic<unsigned long> retained_sessions_;
std::atomic<unsigned long> retained_listeners_;
std::atomic<unsigned long> udp_binds_;
std::atomic<unsigned long> active_tcp_streams_;
```

Add `try_acquire_*` and `release_*` methods that use compare-exchange loops so counters never exceed `limits_`. Enforce:

- retained sessions in `PortTunnelService::create_session`.
- retained TCP listeners before insertion into `session->tcp_listeners`.
- UDP binds before insertion into retained `session->udp_binds` or transport-owned `udp_sockets_`.
- TCP connect streams and accepted TCP streams before insertion into `tcp_streams_`.

Release budgets exactly once in close paths by adding budget flags to resource structs:

```cpp
bool retained_budget_acquired;
bool active_stream_budget_acquired;
bool udp_bind_budget_acquired;
```

Use existing close helpers (`mark_retained_listener_closed`, `mark_tcp_stream_closed`, `mark_udp_socket_closed`) to release associated service counters when resources close. For session expiry and terminal cleanup, close helpers must release all retained listener, UDP bind, and active stream budgets.

- [ ] **Step 5: Run post-change verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h \
  crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "fix: enforce cpp port tunnel resource limits"
```

### Task 3: C++ Tunnel I/O Liveness And Documentation

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** TDD for timeout behavior, existing tests + targeted verification for docs/build
Reason: A partial upgraded tunnel frame must not block a daemon worker forever. The behavior is observable with a low configured timeout and a deliberately incomplete frame.

- [ ] **Step 1: Write failing partial-frame timeout test**

In `tests/test_server_streaming.cpp`, configure `port_forward_tunnel_io_timeout_ms = 50`, upgrade a tunnel, send the preface and only part of a frame header, then assert the server thread exits after the timeout:

```cpp
send_preface(client_socket.get());
const unsigned char partial_header[2] = {static_cast<unsigned char>(PortTunnelFrameType::TcpData), 0U};
send_all_bytes(client_socket.get(), reinterpret_cast<const char*>(partial_header), sizeof(partial_header));
server_thread.join();
```

The test should fail before implementation because `read_exact` blocks indefinitely.

- [ ] **Step 2: Run red check**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: FAIL or hang at the new timeout test before implementation. Interrupt if it hangs beyond 10 seconds.

- [ ] **Step 3: Implement socket I/O timeouts**

Update `PortTunnelConnection::read_exact` to wait for readability with `service_->limits().tunnel_io_timeout_ms` before each `recv`. If readability times out, mark the connection closed and return `false`.

Update `send_frame`/`send_all_bytes` path only as needed to avoid indefinite upgraded-tunnel writes: set socket send and receive timeouts on the upgraded socket after the HTTP `101` response using the same configured timeout, preserving existing HTTP request timeout handling before upgrade.

- [ ] **Step 4: Update docs/config example**

Document new C++ keys in `crates/remote-exec-daemon-cpp/README.md` and add commented defaults to `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`:

```ini
# port_forward_max_retained_sessions = 64
# port_forward_max_retained_listeners = 64
# port_forward_max_udp_binds = 64
# port_forward_max_active_tcp_streams = 1024
# port_forward_max_tunnel_queued_bytes = 8388608
# port_forward_tunnel_io_timeout_ms = 30000
```

- [ ] **Step 5: Run post-change verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp \
  crates/remote-exec-daemon-cpp/src/http_connection.cpp \
  crates/remote-exec-daemon-cpp/README.md \
  crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "fix: bound cpp port tunnel io waits"
```

### Task 4: Rust Broker Configured Limits And TCP Accounting

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** TDD
Reason: Broker limits are externally visible through forward behavior and status counters. Tests should prove configured values, not fixed constants, drive enforcement.

- [ ] **Step 1: Write failing broker limit tests**

Add broker integration tests that configure:

```toml
[port_forward_limits]
max_active_tcp_streams_per_forward = 1
max_pending_tcp_bytes_per_stream = 64
max_pending_tcp_bytes_per_forward = 64
max_udp_peers_per_forward = 1
max_tunnel_queued_bytes = 4096
```

Assert:

- two simultaneous TCP accepted streams do not both become active when the limit is `1`;
- pending pre-connect TCP data above `64` bytes is rejected or closes the stream;
- a second UDP peer/connector is refused when `max_udp_peers_per_forward = 1`;
- `active_tcp_streams` increments on `TcpAccept` and decrements on stream close.

- [ ] **Step 2: Run red check**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: FAIL because bridge code still uses hard-coded constants and does not increment active TCP streams on accept.

- [ ] **Step 3: Thread broker limits into runtime**

Add configured limit fields to `ForwardRuntime`, populated by `open_forward`, and replace fixed constants in `tcp_bridge.rs` and `udp_bridge.rs` with those runtime values:

```rust
max_active_tcp_streams_per_forward: u64,
max_pending_tcp_bytes_per_stream: usize,
max_pending_tcp_bytes_per_forward: usize,
max_udp_peers_per_forward: usize,
max_tunnel_queued_bytes: usize,
```

On `TcpAccept`, try to reserve one active stream before sending `TcpConnect`. If the limit is reached, close the accepted listen stream and do not open the connect stream. Release the active stream once both sides are removed or a pending connect is abandoned.

- [ ] **Step 4: Run post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/mod.rs \
  crates/remote-exec-broker/src/port_forward/supervisor.rs \
  crates/remote-exec-broker/src/port_forward/tcp_bridge.rs \
  crates/remote-exec-broker/src/port_forward/udp_bridge.rs \
  crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "fix: enforce broker port forward stream limits"
```

### Task 5: Rust Broker Reconnect Limit And State Semantics

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** TDD
Reason: State semantics are visible through `list_forward_ports`, and reconnect budgets prevent reliability collapse when many forwards degrade at once.

- [ ] **Step 1: Write failing phase and reconnect tests**

Add tests proving:

- if one side is `Ready` and the other side is `Reconnecting`, global `phase` remains `Reconnecting`;
- `phase` returns to `Ready` only when both sides are `Ready`;
- when `max_reconnecting_forwards = 1`, a second forward trying to enter reconnecting state is marked `Failed` with a limit error.

- [ ] **Step 2: Run red check**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: FAIL because `mark_ready` sets global phase to `Ready` unconditionally and reconnect count is not enforced.

- [ ] **Step 3: Implement derived state and reconnect permits**

Change store updates to derive phase from both side health:

```rust
fn derive_phase(entry: &ForwardPortEntry) -> ForwardPortPhase {
    if entry.listen_state.health == ForwardPortSideHealth::Failed
        || entry.connect_state.health == ForwardPortSideHealth::Failed {
        ForwardPortPhase::Failed
    } else if entry.listen_state.health == ForwardPortSideHealth::Closed
        && entry.connect_state.health == ForwardPortSideHealth::Closed {
        ForwardPortPhase::Closed
    } else if entry.listen_state.health == ForwardPortSideHealth::Ready
        && entry.connect_state.health == ForwardPortSideHealth::Ready {
        ForwardPortPhase::Ready
    } else {
        ForwardPortPhase::Reconnecting
    }
}
```

Track reconnecting forward IDs in `PortForwardStore`. `mark_reconnecting` should reserve a reconnect slot if the entry is not already reconnecting. If the limit is reached, mark that forward failed and return a signal to the caller so its loop exits. Release a slot when both sides become ready, the forward fails, closes, or is drained.

- [ ] **Step 4: Run post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/store.rs \
  crates/remote-exec-broker/src/port_forward/tcp_bridge.rs \
  crates/remote-exec-broker/src/port_forward/udp_bridge.rs \
  crates/remote-exec-broker/src/port_forward/supervisor.rs \
  crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "fix: derive broker port forward state"
```

### Task 6: Rust Host And Daemon Tunnel Resource Limits

**Files:**
- Modify: `crates/remote-exec-host/src/lib.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session_store.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-host port_tunnel_tests`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`

**Testing approach:** TDD
Reason: Host-side enforcement has direct tunnel-frame and daemon-route behavior: low configured limits should produce bounded, deterministic `Error` frames or rejected upgrade attempts.

- [ ] **Step 1: Write failing host/daemon tests**

Add tests for:

- `TunnelReady.limits` reflects custom `HostPortForwardLimits`;
- low `max_retained_listeners` rejects a second retained listener with `port_tunnel_limit_exceeded`;
- low `max_udp_binds` rejects a second bind;
- low `max_active_tcp_streams` rejects a second TCP connect/accept stream;
- low `max_tunnel_connections` rejects or closes a second concurrent upgraded tunnel;
- low `max_tunnel_queued_bytes` returns a backpressure error rather than allowing unbounded queued frame bytes.

- [ ] **Step 2: Run red checks**

Run: `cargo test -p remote-exec-host port_tunnel_tests`
Expected: FAIL because host limits other than retained session count are not enforced.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: FAIL for the tunnel connection limit test.

- [ ] **Step 3: Implement runtime limit guards**

Add a shared port-forward runtime limiter to host state with semaphores or atomic counters for:

- tunnel connections;
- retained listeners;
- UDP binds;
- active TCP streams;
- queued tunnel bytes.

Acquire before resource insertion/spawn and release on cancellation/drop. Wrap tunnel `tx` sending with byte accounting: reserve `frame.encoded_len()` before enqueue, release after writer consumes the frame or when send fails. On budget exhaustion, send an `Error` frame with code `port_tunnel_limit_exceeded` and a deterministic message.

- [ ] **Step 4: Run post-change verification**

Run: `cargo test -p remote-exec-host port_tunnel_tests`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host/src/lib.rs \
  crates/remote-exec-host/src/port_forward/mod.rs \
  crates/remote-exec-host/src/port_forward/session_store.rs \
  crates/remote-exec-host/src/port_forward/tcp.rs \
  crates/remote-exec-host/src/port_forward/udp.rs \
  crates/remote-exec-host/src/port_forward/tunnel.rs \
  crates/remote-exec-daemon/src/port_forward.rs \
  crates/remote-exec-daemon/tests/port_forward_rpc.rs
git commit -m "fix: enforce daemon port tunnel limits"
```

### Task 7: Final Documentation And Verification Sweep

**Files:**
- Modify: `README.md`
- Modify: `configs/daemon.example.toml`
- Modify: `configs/broker.example.toml`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing tests + full quality gate
Reason: This task reconciles operator-facing docs with implemented behavior and runs the project quality gate for cross-cutting port-forward changes.

- [ ] **Step 1: Update operator documentation**

Document that v4 tunnel limits are enforced on both Rust and C++ daemon paths, including C++’s narrower plain-HTTP transport and configured timeout behavior. Ensure docs name the `X-Remote-Exec-Port-Tunnel-Version: 4` header exactly.

- [ ] **Step 2: Run formatting and focused C++ gate**

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 3: Run full Rust gate**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add README.md configs/daemon.example.toml configs/broker.example.toml skills/using-remote-exec-mcp/SKILL.md
git commit -m "docs: document port forward limit enforcement"
```

