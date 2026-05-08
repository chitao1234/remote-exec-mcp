# Port Forward V4 Followup Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Finish the remaining v4 port-forward reliability fixes by consuming daemon-advertised limits, correcting TCP stream accounting, bringing C++ close/worker/connect semantics to truthful v4 parity, and bounding outbound TCP connect attempts on both implementations.

**Architecture:** The broker continues to own public `forward_id` state and must compute the per-forward runtime limit summary from broker configuration plus both daemon `TunnelReady.limits` values. Rust host and C++ daemon keep daemon-local enforcement, but outbound connect attempts become bounded operations and C++ stream success frames are emitted only after the daemon has all resources needed to service the stream. `TunnelClose` is an operator/graceful close, not an internal terminal failure.

**Tech Stack:** Rust 2024, Tokio, `remote-exec-proto` v4 tunnel frames, broker MCP integration tests with stub daemons, Rust host tunnel tests, C++11-compatible daemon code, POSIX C++ streaming tests and Make targets.

---

### Task 0: Commit This Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-09-port-forward-v4-followup-hardening.md`
- Test/Verify: `git diff -- docs/superpowers/plans/2026-05-09-port-forward-v4-followup-hardening.md`

**Testing approach:** no new tests needed
Reason: This is a planning artifact. Reviewing the diff is the useful verification.

- [ ] **Step 1: Review the plan diff**

Run: `git diff -- docs/superpowers/plans/2026-05-09-port-forward-v4-followup-hardening.md`
Expected: The plan lists the remaining broker, C++, Rust, docs, and verification tasks and keeps C++ as first-class work.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/plans/2026-05-09-port-forward-v4-followup-hardening.md
git commit -m "plan: outline port forward v4 followup hardening"
```

### Task 1: Broker Effective Daemon Limit Consumption

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/limits.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_reports_effective_daemon_limits -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** TDD
Reason: The public observable behavior is exact: `forward_ports open/list` must report broker limits clamped by both daemon-advertised `TunnelReady.limits`.

- [ ] **Step 1: Write the failing public broker test**

Add a stub-daemon control helper that rewrites `TunnelReady.limits` for target `builder-a` and add test `forward_ports_reports_effective_daemon_limits`:

```rust
#[tokio::test]
async fn forward_ports_reports_effective_daemon_limits() {
    let fixture = support::spawners::spawn_broker_with_local_and_stub_port_forward_version_and_port_forward_limits(
        4,
        r#"
max_active_tcp_streams_per_forward = 100
max_pending_tcp_bytes_per_stream = 4096
max_pending_tcp_bytes_per_forward = 8192
max_udp_peers_per_forward = 100
max_tunnel_queued_bytes = 10000
max_reconnecting_forwards = 7
"#,
    )
    .await;
    support::stub_daemon::override_tunnel_ready_limits(
        &fixture.daemon_state,
        remote_exec_proto::port_tunnel::TunnelLimitSummary {
            max_active_tcp_streams: 3,
            max_udp_peers: 5,
            max_queued_bytes: 4096,
        },
    )
    .await;

    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let limits = &open.structured_content["forwards"][0]["limits"];
    assert_eq!(limits["max_active_tcp_streams"], 3);
    assert_eq!(limits["max_udp_peers"], 5);
    assert_eq!(limits["max_tunnel_queued_bytes"], 4096);
    assert_eq!(limits["max_pending_tcp_bytes_per_stream"], 4096);
    assert_eq!(limits["max_pending_tcp_bytes_per_forward"], 8192);
    assert_eq!(limits["max_reconnecting_forwards"], 7);
}
```

- [ ] **Step 2: Run the red check**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_reports_effective_daemon_limits -- --nocapture`
Expected: FAIL because the broker still reports broker-configured active TCP, UDP peer, and queued-byte limits.

- [ ] **Step 3: Implement effective limit calculation**

Change `open_listen_session` and `open_data_tunnel` to return decoded `TunnelReadyMeta` limits:

```rust
struct OpenListenSession {
    tunnel: Arc<PortTunnel>,
    session_id: String,
    resume_timeout: Duration,
    limits: TunnelLimitSummary,
}

pub(super) struct OpenDataTunnel {
    pub(super) tunnel: Arc<PortTunnel>,
    pub(super) limits: TunnelLimitSummary,
}
```

Add a helper in `limits.rs` that clamps broker summary fields by daemon limits while preserving broker-only pending TCP and reconnect limits:

```rust
pub fn effective_forward_limits(
    broker: ForwardPortLimitSummary,
    listen: TunnelLimitSummary,
    connect: TunnelLimitSummary,
) -> ForwardPortLimitSummary {
    ForwardPortLimitSummary {
        max_active_tcp_streams: broker
            .max_active_tcp_streams
            .min(listen.max_active_tcp_streams)
            .min(connect.max_active_tcp_streams),
        max_udp_peers: broker
            .max_udp_peers
            .min(listen.max_udp_peers)
            .min(connect.max_udp_peers),
        max_pending_tcp_bytes_per_stream: broker.max_pending_tcp_bytes_per_stream,
        max_pending_tcp_bytes_per_forward: broker.max_pending_tcp_bytes_per_forward,
        max_tunnel_queued_bytes: broker
            .max_tunnel_queued_bytes
            .min(listen.max_queued_bytes)
            .min(connect.max_queued_bytes),
        max_reconnecting_forwards: broker.max_reconnecting_forwards,
    }
}
```

Open initial tunnels using the broker queue limit, compute `effective_limits`, then build `ListenSessionControl`, `ForwardRuntime`, and `ForwardPortEntry` with the effective summary. Update reconnect/open-data call sites to access `.tunnel`.

- [ ] **Step 4: Update live docs**

In `README.md` and `skills/using-remote-exec-mcp/SKILL.md`, state that `forwards[].limits` are the effective per-forward ceilings computed from broker configuration and the listen/connect daemon `TunnelReady.limits`; pending TCP byte and reconnect ceilings are broker-owned.

- [ ] **Step 5: Run post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_reports_effective_daemon_limits -- --nocapture`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/supervisor.rs \
  crates/remote-exec-broker/src/port_forward/limits.rs \
  crates/remote-exec-broker/tests/support/stub_daemon.rs \
  crates/remote-exec-broker/tests/support/mod.rs \
  crates/remote-exec-broker/tests/mcp_forward_ports.rs \
  README.md \
  skills/using-remote-exec-mcp/SKILL.md
git commit -m "fix: honor daemon port tunnel limits in broker"
```

### Task 2: Broker Pending TCP Close Accounting

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Test/Verify: `cargo test -p remote-exec-broker --lib port_forward::tcp_bridge::tests::listen_close_before_connect_ready_releases_active_stream -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --lib port_forward::tcp_bridge::tests -- --nocapture`

**Testing approach:** TDD
Reason: A unit-level tunnel script can reproduce the exact race: listen-side `Close` before connect-side `TcpConnectOk` must release broker active-stream state immediately and must not double-release later.

- [ ] **Step 1: Write the failing unit test**

In `tcp_bridge.rs` tests, add `listen_close_before_connect_ready_releases_active_stream`:

```rust
#[tokio::test]
async fn listen_close_before_connect_ready_releases_active_stream() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime.store.insert(test_record(&runtime, "127.0.0.1:10000")).await;
    let cancel = runtime.cancel.clone();

    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &Frame {
        frame_type: FrameType::TcpAccept,
        flags: 0,
        stream_id: 11,
        meta: serde_json::to_vec(&serde_json::json!({"listener_stream_id": 1})).unwrap(),
        data: Vec::new(),
    }).await.unwrap();
    connect_io.wait_for_written_frame(FrameType::TcpConnect, 1).await;
    write_frame(&mut listen_daemon_side, &Frame {
        frame_type: FrameType::Close,
        flags: 0,
        stream_id: 11,
        meta: Vec::new(),
        data: Vec::new(),
    }).await.unwrap();
    connect_io.wait_for_written_frame(FrameType::Close, 1).await;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
            if entries[0].active_tcp_streams == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("listen close should release active stream before TcpConnectOk");

    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpConnectOk,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: Vec::new(),
    });
    tokio::task::yield_now().await;
    let entries = runtime.store.list(&filter_one("fwd_test")).await;
    assert_eq!(entries[0].active_tcp_streams, 0);
    assert_eq!(entries[0].dropped_tcp_streams, 0);

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stop after cancellation")
        .unwrap()
        .expect("pending close should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}
```

- [ ] **Step 2: Run the red check**

Run: `cargo test -p remote-exec-broker --lib port_forward::tcp_bridge::tests::listen_close_before_connect_ready_releases_active_stream -- --nocapture`
Expected: FAIL because active stream accounting remains pinned until `TcpConnectOk`.

- [ ] **Step 3: Implement immediate release for not-ready close**

In the listen-side `FrameType::Close` branch, when the paired connect stream exists but is not ready:

```rust
if let Some(mut stream) = state.connect_streams.remove(&connect_stream_id) {
    release_pending_budget(&mut state.pending_budget, &mut stream);
    let _ = connect_tunnel.close_stream(connect_stream_id).await;
    release_active_tcp_stream(runtime).await;
}
```

Do not queue a `Close` frame on a removed not-ready stream. Late `TcpConnectOk`, data, or close frames for that connect stream are ignored by the existing missing-stream checks.

- [ ] **Step 4: Run post-change verification**

Run: `cargo test -p remote-exec-broker --lib port_forward::tcp_bridge::tests::listen_close_before_connect_ready_releases_active_stream -- --nocapture`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --lib port_forward::tcp_bridge::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/tcp_bridge.rs
git commit -m "fix: release pending tcp stream on early close"
```

### Task 3: C++ Graceful Close And Worker Exposure Ordering

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** TDD plus integration verification
Reason: C++ emits observable tunnel frames. Worker-limit tests can prove that success frames are not emitted before read/loop workers are available, and graceful close can be proven by reusing a listener endpoint immediately after explicit `TunnelClose`.

- [ ] **Step 1: Write failing C++ tests**

Add these assertions to `test_server_streaming.cpp`:

```cpp
static void assert_tunnel_close_is_graceful_for_retained_listener(const fs::path& root) {
    AppState state;
    initialize_state(state, root);
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);
    send_tunnel_frame(client_socket.get(), json_frame(PortTunnelFrameType::TunnelOpen, 0U, tunnel_open_meta("listen", "tcp", 1ULL)));
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelReady);
    send_tunnel_frame(client_socket.get(), json_frame(PortTunnelFrameType::TcpListen, 1U, Json{{"endpoint", "127.0.0.1:0"}}));
    const PortTunnelFrame listen_ok = read_tunnel_frame(client_socket.get());
    assert(listen_ok.type == PortTunnelFrameType::TcpListenOk);
    const std::string endpoint = Json::parse(listen_ok.meta).at("endpoint").get<std::string>();
    send_tunnel_frame(client_socket.get(), json_frame(PortTunnelFrameType::TunnelClose, 0U, Json{{"forward_id", "fwd_cpp_test"}, {"generation", 1ULL}, {"reason", "operator_close"}}));
    assert(read_tunnel_frame(client_socket.get()).type == PortTunnelFrameType::TunnelClosed);
    close_tunnel(&client_socket, &server_thread);
    UniqueSocket rebound(bind_port_forward_socket(endpoint, "tcp"));
    assert(rebound.valid());
}

static void assert_tcp_connect_worker_limit_errors_before_success(const fs::path& root) {
    PortForwardLimitConfig limits = default_port_forward_limit_config();
    limits.max_worker_threads = 1UL;
    AppState state;
    initialize_state_with_port_forward_limits(state, root, limits);
    UniqueSocket listener(bind_port_forward_socket("127.0.0.1:0", "tcp"));
    const std::string endpoint = socket_local_endpoint(listener.get());
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);
    send_tunnel_frame(client_socket.get(), json_frame(PortTunnelFrameType::TcpConnect, 1U, Json{{"endpoint", endpoint}}));
    const PortTunnelFrame response = read_tunnel_frame(client_socket.get());
    assert(response.type == PortTunnelFrameType::Error);
    assert(response.stream_id == 1U);
    const Json meta = Json::parse(response.meta);
    assert(meta.at("code").get<std::string>() == "port_tunnel_limit_exceeded");
    assert(!meta.at("fatal").get<bool>());
    close_tunnel(&client_socket, &server_thread);
}
```

Also update the existing worker-limit assertion so listener worker shortage reports a nonfatal stream error instead of a fatal terminal error.

- [ ] **Step 2: Run the red check**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: FAIL because explicit `TunnelClose` closes retained sessions as terminal, and TCP connect can send `TcpConnectOk` before failing to spawn the read worker.

- [ ] **Step 3: Implement C++ close and worker ordering**

Add `GracefulClose` to `PortTunnelCloseMode` and update `close_current_session`:

```cpp
enum class PortTunnelCloseMode {
    RetryableDetach,
    GracefulClose,
    TerminalFailure,
};
```

Use `GracefulClose` in `PortTunnelConnection::tunnel_close`. Keep `RetryableDetach` for transport loss and `TerminalFailure` for malformed/fatal tunnel failure.

Replace `fail_worker_limit` terminal teardown with a nonfatal per-stream error helper for resource-specific worker shortage:

```cpp
void PortTunnelConnection::send_worker_limit(uint32_t stream_id) {
    send_error(stream_id, "port_tunnel_limit_exceeded", "port tunnel worker limit reached");
}
```

For retained TCP listeners, transport-owned TCP listeners, retained/transport UDP binds, TCP connect read workers, and accepted TCP read workers:

- bind/create resource
- reserve daemon resource counter where applicable
- spawn the required worker before sending `TcpListenOk`, `UdpBindOk`, `TcpConnectOk`, or `TcpAccept`
- if spawn fails, close/remove that stream resource, release its budget, send nonfatal `port_tunnel_limit_exceeded`, and keep the tunnel/session alive

- [ ] **Step 4: Run post-change verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h \
  crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "fix: make cpp tunnel worker failures stream local"
```

### Task 4: C++ Active TCP Connect Accounting And Connect Timeout

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Modify: `crates/remote-exec-daemon-cpp/include/port_forward_socket_ops.h`
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-config`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** TDD where practical plus build verification
Reason: Config parsing is directly testable. The active-stream accounting regression can be covered with an existing connected socket; blackhole network timeout behavior is platform dependent, so the implementation is verified by unit-level config tests and C++ build/runtime tests.

- [ ] **Step 1: Write failing C++ config and accounting tests**

In `test_config.cpp`, add `port_forward_connect_timeout_ms = 7000` to the parsed config fixture, assert `config.port_forward_limits.connect_timeout_ms == 7000UL`, and add zero-value rejection for `port_forward_connect_timeout_ms`.

In `test_server_streaming.cpp`, adjust `assert_active_tcp_stream_limit_is_enforced_and_released` to prove the active stream counter is acquired only after a successful connect by first attempting `TcpConnect` to a free but closed loopback endpoint and then connecting successfully to a live listener under `max_active_tcp_streams = 1`.

- [ ] **Step 2: Run red checks**

Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
Expected: FAIL because `connect_timeout_ms` does not exist or is not parsed.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: FAIL if active TCP budget is still consumed before connect failure under the new assertion.

- [ ] **Step 3: Implement C++ timeout and post-connect accounting**

Add to `PortForwardLimitConfig`:

```cpp
unsigned long connect_timeout_ms;
```

Default it to `10000UL`, parse `port_forward_connect_timeout_ms`, and validate it is greater than zero.

Change `connect_port_forward_socket` to accept a timeout:

```cpp
SOCKET connect_port_forward_socket(
    const std::string& endpoint,
    const std::string& protocol,
    unsigned long timeout_ms
);
```

Implement TCP timeout with nonblocking `connect`, `select` for writability, `SO_ERROR`, and restoration to blocking mode before returning. Use the existing blocking path for UDP if needed.

In `PortTunnelConnection::tcp_connect`, call `connect_port_forward_socket(endpoint, "tcp", service_->limits().connect_timeout_ms)` first, then acquire `active_tcp_streams`, insert the stream, spawn the read worker, and only then send `TcpConnectOk`. If budget or worker reservation fails after the socket connects, close the socket and send a nonfatal stream error.

- [ ] **Step 4: Update C++ docs/config example**

Document `port_forward_connect_timeout_ms` in `crates/remote-exec-daemon-cpp/README.md` and `config/daemon-cpp.example.ini`, and clarify that active TCP stream limits count established streams, not pending outbound connect attempts.

- [ ] **Step 5: Run post-change verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/config.h \
  crates/remote-exec-daemon-cpp/include/port_forward_socket_ops.h \
  crates/remote-exec-daemon-cpp/src/config.cpp \
  crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp \
  crates/remote-exec-daemon-cpp/tests/test_config.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp \
  crates/remote-exec-daemon-cpp/README.md \
  crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini
git commit -m "fix: bound cpp tcp connect attempts"
```

### Task 5: Rust Host TCP Connect Timeout

**Files:**
- Modify: `crates/remote-exec-host/src/config/mod.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Modify: `configs/daemon.example.toml`
- Modify: `README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `cargo test -p remote-exec-host port_tunnel_tests`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`

**Testing approach:** TDD where deterministic
Reason: Config validation and ordinary connect behavior are deterministic. True blackhole timeout timing is network-dependent, so the behavioral guarantee is implemented through `tokio::time::timeout` and covered by focused host/daemon tests that the new config is accepted and existing tunnel connect paths still work.

- [ ] **Step 1: Write failing Rust config/behavior tests**

Add `connect_timeout_ms` to `HostPortForwardLimits` construction in host tests and daemon config fixture tests, asserting:

```rust
assert_eq!(config.port_forward_limits.connect_timeout_ms, 10_000);
```

Add a daemon config test or extend an existing config parse test to accept:

```toml
[port_forward_limits]
connect_timeout_ms = 7000
```

and reject `connect_timeout_ms = 0` with message `port_forward_limits.connect_timeout_ms must be greater than zero`.

- [ ] **Step 2: Run red checks**

Run: `cargo test -p remote-exec-host port_tunnel_tests -- --nocapture`
Expected: FAIL if struct initializers and assertions reference the new field before implementation.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: FAIL if config parsing references the new field before implementation.

- [ ] **Step 3: Implement Rust connect timeout**

Add `connect_timeout_ms: u64` to `HostPortForwardLimits`, default it to `10_000`, validate it is greater than zero, and update all struct literals.

In both `tunnel_tcp_connect` and `tunnel_tcp_connect_transport_owned`, replace direct connects with:

```rust
let connect_timeout = std::time::Duration::from_millis(
    tunnel.state.config.port_forward_limits.connect_timeout_ms,
);
let stream = tokio::time::timeout(connect_timeout, TcpStream::connect(endpoint.as_str()))
    .await
    .map_err(|_| rpc_error("port_connect_failed", "tcp connect timed out"))?
    .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?;
```

- [ ] **Step 4: Update Rust docs/config example**

Document `connect_timeout_ms` under `[port_forward_limits]` in `configs/daemon.example.toml`, `README.md`, and `skills/using-remote-exec-mcp/SKILL.md`.

- [ ] **Step 5: Run post-change verification**

Run: `cargo test -p remote-exec-host port_tunnel_tests -- --nocapture`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-host/src/config/mod.rs \
  crates/remote-exec-host/src/port_forward/tcp.rs \
  crates/remote-exec-host/src/port_forward/mod.rs \
  crates/remote-exec-daemon/tests/port_forward_rpc.rs \
  configs/daemon.example.toml \
  README.md \
  skills/using-remote-exec-mcp/SKILL.md
git commit -m "fix: bound rust tcp connect attempts"
```

### Task 6: Final Verification

**Files:**
- Verify only unless earlier task failures require fixes.
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Test/Verify: `git diff --check`

**Testing approach:** existing tests + full quality gate
Reason: The changes cross broker, Rust daemon, C++ daemon, docs, and public behavior, so the focused tests plus workspace quality gate are the appropriate completion evidence.

- [ ] **Step 1: Run focused forwarding checks**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-daemon --test port_forward_rpc
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: All commands exit 0.

- [ ] **Step 2: Run workspace quality gate**

Run:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
```

Expected: All commands exit 0.

- [ ] **Step 3: Commit final fixups if needed**

If formatting, clippy, docs, or test-fix edits were required after Task 5, commit them:

```bash
git add <changed-files>
git commit -m "chore: finish port forward v4 followup hardening"
```

If no files changed, do not create an empty commit.
