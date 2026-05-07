# Port Forward Reconnect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Make `forward_ports` survive transient broker-daemon transport drops when the daemon stays alive, while preserving only forward-level listener state and future connections.

**Architecture:** Upgrade the internal port-tunnel protocol from tunnel-lifetime ownership to resumable listen-side session ownership with a bounded reconnect window. The broker keeps the same public `forward_id`, retries only transport-class failures, resumes the preserved listen-side session, and recreates non-resumable connect-side stream state as needed.

**Tech Stack:** Rust 2024, Tokio async I/O, reqwest upgrade sockets, Hyper/Axum HTTP/1.1 upgrades, Rust shared host runtime, C++11 sockets/threads, C++ daemon HTTP upgrade tunnel, Rust/C++ integration tests.

---

## File Structure

- `crates/remote-exec-proto/src/port_tunnel.rs`: bump tunnel protocol header version, add tunnel session control frame types, and add session-control codec tests.
- `crates/remote-exec-proto/src/rpc.rs`: bump `TargetInfoResponse.port_forward_protocol_version` usage contract from `2` to `3`.
- `crates/remote-exec-host/src/lib.rs` and related state helpers: expose host-level target info with reconnect-capable protocol version and any shared runtime state hooks needed by the resumable tunnel service.
- `crates/remote-exec-host/src/port_forward.rs`: add resumable listen-side session registry, session attach/resume control flow, detach cleanup rules, and host tunnel tests.
- `crates/remote-exec-daemon/src/port_forward.rs`: keep the existing route, but route upgraded connections into the resumable host tunnel service.
- `crates/remote-exec-daemon/src/lib.rs`: report reconnect-capable port-forward protocol version in target info.
- `crates/remote-exec-broker/src/state.rs`: require reconnect-capable remote targets for forwarding.
- `crates/remote-exec-broker/src/daemon_client.rs`: keep remote tunnel open logic but send the upgraded header version and expose enough transport errors to drive reconnect.
- `crates/remote-exec-broker/src/port_forward.rs`: add listen-session open/resume metadata, transport-class reconnect handling, connect-side tunnel recreation, and updated broker unit tests.
- `crates/remote-exec-broker/tests/support/stub_daemon.rs`: add a test-only reconnect-capable tunnel stub and a way to force-close only the upgraded tunnel transport.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs`: add broker integration coverage for reconnect success, reconnect timeout, and non-retryable errors.
- `tests/e2e/support/mod.rs` and related daemon fixtures: add support for forcing a live daemon to drop upgraded port-tunnel connections without restarting the daemon process.
- `tests/e2e/multi_target.rs`: add end-to-end reconnect tests for TCP and UDP, and adjust broker-crash cleanup expectations to allow grace-window cleanup.
- `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`: add the new control frame enum values.
- `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`: update frame codec tests if needed for new frame types.
- `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`: add resumable session registry, detach/resume logic, listener retention, and expiry cleanup.
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp` and relevant C++ tunnel tests: add protocol version `3` assertions and reconnect behavior coverage.
- Documentation: `README.md`, `crates/remote-exec-daemon-cpp/README.md`, `skills/using-remote-exec-mcp/SKILL.md`, and the existing port-forward design/spec references if they describe transport-drop cleanup as immediate.

---

### Task 1: Upgrade the Tunnel Protocol Contract

**Files:**
- Modify: `crates/remote-exec-proto/src/port_tunnel.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-host/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/state.rs`
- Modify: `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `cargo test -p remote-exec-proto port_tunnel --lib`, `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_targets_without_tunnel_protocol_version -- --exact`, `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** TDD
Reason: The versioning and frame-surface changes are isolated and should be locked down before any runtime reconnect logic is introduced.

- [ ] **Step 1: Add failing protocol-version and control-frame tests**

Update the Rust proto tests in `crates/remote-exec-proto/src/port_tunnel.rs` to cover the new wire contract:

```rust
#[tokio::test]
async fn session_control_frames_round_trip() {
    let (mut client, mut server) = tokio::io::duplex(1024);
    let writer = tokio::spawn(async move {
        write_frame(
            &mut client,
            &Frame {
                frame_type: FrameType::SessionResume,
                flags: 0,
                stream_id: 0,
                meta: br#"{"session_id":"sess_123"}"#.to_vec(),
                data: Vec::new(),
            },
        )
        .await
    });

    let frame = read_frame(&mut server).await.unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(frame.frame_type, FrameType::SessionResume);
    assert_eq!(frame.stream_id, 0);
    assert_eq!(frame.meta, br#"{"session_id":"sess_123"}"#);
}

#[test]
fn target_info_defaults_to_protocol_zero() {
    let info: crate::rpc::TargetInfoResponse = serde_json::from_value(serde_json::json!({
        "target": "old",
        "daemon_version": "0",
        "daemon_instance_id": "i",
        "hostname": "h",
        "platform": "linux",
        "arch": "x86_64",
        "supports_pty": false,
        "supports_image_read": false,
        "supports_port_forward": true
    }))
    .unwrap();
    assert_eq!(info.port_forward_protocol_version, 0);
}
```

Update `crates/remote-exec-broker/tests/mcp_forward_ports.rs` so the capability rejection test now expects:

```rust
assert!(error.contains("does not support port forward protocol version 3"));
```

Update `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp` so target info assertions require:

```cpp
assert(info.at("port_forward_protocol_version").get<int>() == 3);
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-proto port_tunnel --lib`
Expected: FAIL because `FrameType::SessionResume` and the updated tunnel header version do not exist yet.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_targets_without_tunnel_protocol_version -- --exact`
Expected: FAIL because the broker still accepts protocol version `2`.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: FAIL because the C++ daemon target info still reports `port_forward_protocol_version = 2`.

- [ ] **Step 3: Implement the protocol and capability bump**

Update `crates/remote-exec-proto/src/port_tunnel.rs`:

```rust
pub const TUNNEL_PROTOCOL_VERSION: &str = "2";

#[repr(u8)]
pub enum FrameType {
    Error = 1,
    Close = 2,
    SessionOpen = 3,
    SessionReady = 4,
    SessionResume = 5,
    SessionResumed = 6,
    TcpListen = 10,
    TcpListenOk = 11,
    TcpAccept = 12,
    TcpConnect = 13,
    TcpConnectOk = 14,
    TcpData = 15,
    TcpEof = 16,
    UdpBind = 30,
    UdpBindOk = 31,
    UdpDatagram = 32,
}
```

Update capability reporting so reconnect-capable endpoints return protocol version `3`:

```rust
pub fn target_info_response(state: &HostRuntimeState, daemon_version: &str) -> TargetInfoResponse {
    TargetInfoResponse {
        target: state.config.target.clone(),
        daemon_version: daemon_version.to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
        hostname: hostname(),
        platform: state.platform.clone(),
        arch: state.arch.clone(),
        supports_pty: state.supports_pty,
        supports_image_read: true,
        supports_transfer_compression: state.supports_transfer_compression,
        supports_port_forward: true,
        port_forward_protocol_version: 3,
    }
}
```

Update broker validation in `crates/remote-exec-broker/src/state.rs`:

```rust
anyhow::ensure!(
    info.supports_port_forward && info.port_forward_protocol_version >= 3,
    "target `{name}` does not support port forward protocol version 3"
);
```

Update the C++ target-info JSON in `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`:

```cpp
{"supports_port_forward", true},
{"port_forward_protocol_version", 3},
```

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-proto port_tunnel --lib`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_targets_without_tunnel_protocol_version -- --exact`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/port_tunnel.rs crates/remote-exec-proto/src/rpc.rs crates/remote-exec-host/src/lib.rs crates/remote-exec-daemon/src/lib.rs crates/remote-exec-broker/src/state.rs crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h crates/remote-exec-daemon-cpp/src/server_route_common.cpp crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
git commit -m "feat: add reconnect-capable port tunnel protocol version"
```

---

### Task 2: Add Resumable Listen-Side Sessions to the Rust Host Tunnel Service

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-host port_tunnel --lib`

**Testing approach:** TDD
Reason: The host tunnel service already runs over `tokio::io::duplex`, which is a strong seam for validating detach/resume behavior before broker integration.

- [ ] **Step 1: Add failing host tunnel resume tests**

Extend `crates/remote-exec-host/src/port_forward.rs` tests with resumable listener coverage:

```rust
#[tokio::test]
async fn tcp_listener_session_can_resume_after_transport_drop() {
    let state = test_state();
    let listen_endpoint = free_loopback_endpoint().await;

    let (listen_bound_endpoint, session_id) = open_resumable_tcp_listener(&state, &listen_endpoint).await;

    drop_first_tunnel_transport();

    let mut resumed = resume_session(&state, &session_id).await;
    let accept = tokio::net::TcpStream::connect(&listen_bound_endpoint).await.unwrap();
    drop(accept);

    let frame = read_frame(&mut resumed).await.unwrap();
    assert_eq!(frame.frame_type, FrameType::TcpAccept);
}

#[tokio::test]
async fn udp_bind_session_can_resume_after_transport_drop() {
    let state = test_state();
    let listen_endpoint = free_loopback_endpoint().await;

    let (listen_bound_endpoint, session_id) = open_resumable_udp_bind(&state, &listen_endpoint).await;

    drop_first_tunnel_transport();

    let mut resumed = resume_session(&state, &session_id).await;
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender.send_to(b"ping", &listen_bound_endpoint).await.unwrap();

    let frame = read_frame(&mut resumed).await.unwrap();
    assert_eq!(frame.frame_type, FrameType::UdpDatagram);
    assert_eq!(frame.data, b"ping");
}

#[tokio::test]
async fn expired_detached_listener_is_released() {
    let state = test_state_with_resume_timeout(Duration::from_millis(100));
    let listen_endpoint = free_loopback_endpoint().await;
    let (bound_endpoint, _session_id) = open_resumable_tcp_listener(&state, &listen_endpoint).await;

    drop_first_tunnel_transport();
    tokio::time::sleep(Duration::from_millis(250)).await;

    wait_until_bindable(&bound_endpoint).await;
}
```

Use small local helper functions in the test module:

```rust
async fn start_tunnel(state: Arc<AppState>) -> tokio::io::DuplexStream;
async fn open_resumable_tcp_listener(state: &Arc<AppState>, endpoint: &str) -> (String, String);
async fn open_resumable_udp_bind(state: &Arc<AppState>, endpoint: &str) -> (String, String);
async fn resume_session(state: &Arc<AppState>, session_id: &str) -> tokio::io::DuplexStream;
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-host port_tunnel --lib`
Expected: FAIL because the host tunnel service does not yet understand session control frames or retain listeners across transport drops.

- [ ] **Step 3: Implement resumable session ownership in the host runtime**

In `crates/remote-exec-host/src/port_forward.rs`, add explicit session state and separate retained listener state from transport-bound stream state:

```rust
struct TunnelSessionStore {
    sessions: Mutex<HashMap<String, Arc<SessionState>>>,
}

struct SessionState {
    id: String,
    attached: AtomicBool,
    resume_deadline: Mutex<Option<Instant>>,
    retained_listener: Mutex<Option<RetainedListener>>,
    retained_udp_bind: Mutex<Option<RetainedUdpBind>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
    next_daemon_stream_id: AtomicU32,
}

enum RetainedListener {
    Tcp { stream_id: u32, listener: Arc<TcpListener> },
}

enum RetainedUdpBind {
    Udp { stream_id: u32, socket: Arc<UdpSocket> },
}
```

Handle session control frames on `stream_id = 0`:

```rust
match frame.frame_type {
    FrameType::SessionOpen => tunnel_session_open(tunnel.clone(), frame).await,
    FrameType::SessionResume => tunnel_session_resume(tunnel.clone(), frame).await,
    FrameType::TcpListen => tunnel_tcp_listen(tunnel.clone(), frame).await,
    FrameType::UdpBind => tunnel_udp_bind(tunnel.clone(), frame).await,
    // existing cases...
}
```

Emit session metadata:

```rust
#[derive(Serialize, Deserialize)]
struct SessionReadyMeta {
    session_id: String,
    resume_timeout_ms: u64,
}
```

On transport loss:

```rust
async fn detach_session(session: &Arc<SessionState>) {
    close_non_resumable_streams(session).await;
    *session.resume_deadline.lock().await = Some(Instant::now() + RESUME_TIMEOUT);
    session.attached.store(false, Ordering::SeqCst);
}
```

Add a small async sweep task or sweep-on-access path that drops expired detached sessions and closes retained listeners or sockets.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-host port_tunnel --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host/src/port_forward.rs
git commit -m "feat: add resumable listen-side host port tunnel sessions"
```

---

### Task 3: Teach the Broker to Open, Resume, and Recover Listen-Side Sessions

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** TDD
Reason: The broker runtime has a clear integration seam through the stub daemon and can validate retryable transport failures versus terminal protocol failures.

- [ ] **Step 1: Add failing broker reconnect tests**

Add reconnect-focused broker integration tests to `crates/remote-exec-broker/tests/mcp_forward_ports.rs`:

```rust
#[tokio::test]
async fn forward_ports_keeps_forward_open_after_listen_tunnel_drop() {
    let (addr, mut state) = spawn_plain_http_stub_daemon().await;
    set_port_forward_support(&mut state, true, 3);
    enable_reconnectable_port_tunnel(&state).await;

    let fixture = broker_fixture_against(addr).await;
    let open = fixture.open_remote_tcp_forward().await;
    let forward_id = forward_id_from(&open);

    force_close_port_tunnel_transport(&state).await;

    let forward = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(forward["status"], "open");

    let endpoint = open["listen_endpoint"].as_str().unwrap();
    tokio::net::TcpStream::connect(endpoint).await.unwrap();
}

#[tokio::test]
async fn forward_ports_fails_when_resume_deadline_expires() {
    let (addr, mut state) = spawn_plain_http_stub_daemon().await;
    set_port_forward_support(&mut state, true, 3);
    enable_reconnectable_port_tunnel(&state).await;
    block_session_resume(&state).await;

    let fixture = broker_fixture_against(addr).await;
    let open = fixture.open_remote_tcp_forward().await;
    let forward_id = forward_id_from(&open);

    force_close_port_tunnel_transport(&state).await;

    let failed = wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(10)).await;
    let error = failed["last_error"].as_str().unwrap_or_default();
    assert!(error.contains("reconnect timed out") || error.contains("resume expired"));
}

#[tokio::test]
async fn forward_ports_does_not_retry_stream_error_frames() {
    let (addr, mut state) = spawn_plain_http_stub_daemon().await;
    set_port_forward_support(&mut state, true, 3);
    enable_reconnectable_port_tunnel(&state).await;
    set_listen_error_after_resume(&state, "port_bind_failed", "boom").await;

    let fixture = broker_fixture_against(addr).await;
    let result = fixture.open_remote_tcp_forward().await;
    assert!(tool_error_text(&result).contains("port_bind_failed"));
}
```

Add test-support hooks in `crates/remote-exec-broker/tests/support/stub_daemon.rs`:

```rust
pub(crate) async fn enable_reconnectable_port_tunnel(state: &StubDaemonState);
pub(crate) async fn force_close_port_tunnel_transport(state: &StubDaemonState);
pub(crate) async fn block_session_resume(state: &StubDaemonState);
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: FAIL because the stub daemon cannot emulate a reconnectable port tunnel and the broker treats tunnel read failures as terminal.

- [ ] **Step 3: Implement broker reconnect handling and stub-daemon support**

In `crates/remote-exec-broker/src/daemon_client.rs`, keep `port_tunnel()` but ensure it sends the bumped header version:

```rust
.header(TUNNEL_PROTOCOL_VERSION_HEADER, TUNNEL_PROTOCOL_VERSION)
```

In `crates/remote-exec-broker/src/port_forward.rs`, add explicit listen-session metadata:

```rust
struct ListenSession {
    side: SideHandle,
    session_id: String,
    listen_endpoint: String,
    listener_stream_id: u32,
}

struct RecoveryPolicy {
    deadline: Instant,
    next_backoff: Duration,
}
```

Open the listen-side tunnel with a session handshake:

```rust
async fn open_listen_session(side: &SideHandle) -> anyhow::Result<(Arc<PortTunnel>, String)> {
    let tunnel = Arc::new(side.port_tunnel().await?);
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionOpen,
            flags: 0,
            stream_id: 0,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    let frame = tunnel.recv().await?;
    let ready: SessionReadyMeta = decode_tunnel_meta(&frame)?;
    Ok((tunnel, ready.session_id))
}
```

Add resume flow:

```rust
async fn resume_listen_session(side: &SideHandle, session_id: &str) -> anyhow::Result<Arc<PortTunnel>> {
    let tunnel = Arc::new(side.port_tunnel().await?);
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionResume,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&serde_json::json!({ "session_id": session_id }))?,
            data: Vec::new(),
        })
        .await?;
    let frame = tunnel.recv().await?;
    anyhow::ensure!(frame.frame_type == FrameType::SessionResumed, "unexpected resume response");
    Ok(tunnel)
}
```

When `listen_tunnel.recv()` fails with transport-class errors, call a bounded reconnect loop:

```rust
async fn reconnect_listen_side(runtime: &mut TcpRuntimeState) -> anyhow::Result<()> {
    while Instant::now() < runtime.recovery.deadline {
        match resume_listen_session(&runtime.listen_side, &runtime.listen_session.session_id).await {
            Ok(tunnel) => {
                runtime.listen_tunnel = tunnel;
                runtime.clear_non_resumable_state().await;
                runtime.connect_tunnel = Arc::new(runtime.connect_side.port_tunnel().await?);
                return Ok(());
            }
            Err(err) if is_retryable_reconnect_error(&err) => {
                tokio::time::sleep(runtime.recovery.next_backoff).await;
                runtime.recovery.bump_backoff();
            }
            Err(err) => return Err(err),
        }
    }
    Err(anyhow::anyhow!("port tunnel reconnect timed out"))
}
```

Keep `ForwardPortEntry.status` public behavior unchanged: `open` during retries, `failed` only after recovery exhausts.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/daemon_client.rs crates/remote-exec-broker/src/port_forward.rs crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "feat: reconnect listen-side port forward tunnels in broker"
```

---

### Task 4: Add Live-Daemon Reconnect E2E Coverage for Rust Daemons

**Files:**
- Modify: `tests/e2e/support/mod.rs`
- Modify: `tests/e2e/multi_target.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** characterization/integration test
Reason: The requirement is specifically about behavior across real broker-daemon transport drops, so the main seam is end-to-end behavior with live fixtures rather than isolated unit tests.

- [ ] **Step 1: Add failing transport-drop test support and e2e tests**

In `tests/e2e/support/mod.rs`, add a daemon-fixture helper that can force-close upgraded port-tunnel connections without restarting the daemon. The helper should be named explicitly so the test reads clearly:

```rust
impl DaemonFixture {
    pub async fn drop_port_tunnels(&self) {
        self.admin_force_drop_port_tunnels().await;
    }
}
```

If there is no existing control surface, add a test-only control route on the daemon fixture harness or wrap the live daemon in a test proxy that tracks upgraded sockets and can close them on demand.

Add e2e tests in `tests/e2e/multi_target.rs`:

```rust
#[tokio::test]
async fn forward_ports_reconnect_after_live_tunnel_drop_and_accept_new_tcp_connections() {
    let mut cluster = support::spawn_cluster().await;
    let echo_addr = support::spawn_tcp_echo().await;

    let open = cluster
        .broker
        .open_tcp_forward("builder-a", "local", "127.0.0.1:0", &echo_addr.to_string())
        .await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    support::wait_for_forward_status_timeout(&cluster.broker, &forward_id, "open", Duration::from_secs(5))
        .await
        .expect("forward should stay open after reconnect");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    stream.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");
}

#[tokio::test]
async fn forward_ports_reconnect_after_live_tunnel_drop_and_relays_future_udp_datagrams() {
    let mut cluster = support::spawn_cluster().await;
    let udp_echo = support::spawn_udp_echo().await;

    let open = cluster
        .broker
        .open_udp_forward("builder-a", "local", "127.0.0.1:0", &udp_echo.to_string())
        .await;
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender.send_to(b"after", &listen_endpoint).await.unwrap();

    let mut buf = [0u8; 16];
    let (read, _) = tokio::time::timeout(Duration::from_secs(5), sender.recv_from(&mut buf)).await.unwrap().unwrap();
    assert_eq!(&buf[..read], b"after");
}
```

Adjust broker-crash cleanup assertions if needed so they allow cleanup within the reconnect grace window instead of assuming immediate release.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: FAIL because there is no way to drop only port-tunnel transports, and the current implementation fails forwards on tunnel loss.

- [ ] **Step 3: Implement live-fixture support and e2e-compatible reconnect behavior**

Build the smallest reliable test hook that can close only the upgraded port-tunnel transport while keeping the daemon alive. If using a proxy, keep the production daemon untouched and add a test fixture with these responsibilities:

```rust
struct TunnelDropProxy {
    listened_addr: SocketAddr,
    daemon_addr: SocketAddr,
    active_port_tunnels: Arc<Mutex<Vec<tokio::sync::oneshot::Sender<()>>>>,
}
```

Proxy behavior:

```rust
if request.path() == "/v1/port/tunnel" && upgrade_requested(&request) {
    track_upgraded_stream();
    allow test hook to close only this upgraded connection later;
}
```

Keep the daemon process and target info stable during the drop so the broker is exercising reconnect, not restart handling.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/e2e/support/mod.rs tests/e2e/multi_target.rs
git commit -m "test: cover live port tunnel reconnect behavior"
```

---

### Task 5: Add Resumable Session Support to the C++ Daemon

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`, `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`

**Testing approach:** characterization/integration test
Reason: The C++ daemon already has substantial socket/thread machinery, so the safest seam is feature-parity testing through its existing route and broker integration coverage.

- [ ] **Step 1: Add failing C++ reconnect tests**

Update or add broker-facing C++ daemon tests in `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`:

```rust
#[tokio::test]
async fn cpp_forward_ports_reconnect_after_tunnel_drop() {
    let fixture = CppDaemonFixture::spawn().await;
    let open = fixture.open_tcp_forward().await;
    let listen_endpoint = open.listen_endpoint();

    fixture.drop_port_tunnels().await;

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    stream.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");
}
```

Add or extend native C++ tests so detached sessions can be resumed before expiry and are released after expiry.

- [ ] **Step 2: Verify the red state**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: FAIL because the C++ daemon does not support session control frames or listener retention.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: FAIL because the broker cannot reconnect a C++ daemon tunnel yet.

- [ ] **Step 3: Implement resumable session retention in the C++ tunnel runtime**

In `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`, split session-owned listeners from transport-owned connection state:

```cpp
struct RetainedTcpListener {
    uint32_t stream_id;
    UniqueSocket listener;
};

struct RetainedUdpBind {
    uint32_t stream_id;
    std::shared_ptr<TunnelUdpSocket> socket_value;
};

struct PortTunnelSession {
    std::string session_id;
    bool attached;
    uint64_t resume_deadline_ms;
    std::map<uint32_t, UniqueSocket> tcp_listeners;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_binds;
    uint32_t next_daemon_stream_id;
};
```

Handle new control frames:

```cpp
case PortTunnelFrameType::SessionOpen:
    session_open(frame);
    break;
case PortTunnelFrameType::SessionResume:
    session_resume(frame);
    break;
```

Detach behavior should keep listeners/binds but close accepted stream state:

```cpp
void PortTunnelConnection::detach_session() {
    close_all_tcp_streams();
    close_all_udp_peer_state();
    session_->attached = false;
    session_->resume_deadline_ms = now_ms() + kResumeTimeoutMs;
}
```

Add expiry cleanup in a small sweep path that runs on resume attempts and after connection termination.

- [ ] **Step 4: Run the focused verification**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp crates/remote-exec-daemon-cpp/src/port_tunnel.cpp crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "feat: add resumable port tunnel sessions to cpp daemon"
```

---

### Task 6: Update Docs and Run the Full Verification Gate

**Files:**
- Modify: `README.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Modify: `docs/superpowers/specs/2026-05-06-port-forward-upgrade-tunnel-design.md`
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `make -C crates/remote-exec-daemon-cpp test-host-transfer`, `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing tests + targeted verification
Reason: This task is documentation plus repository-wide validation after feature work is complete.

- [ ] **Step 1: Update the user-facing and operator-facing docs**

Update `README.md` to describe:

```md
- `forward_ports` survives transient broker-daemon transport disconnects when the daemon stays alive.
- Existing TCP connections and UDP per-peer connector state are not preserved.
- Unexpected broker loss may delay remote listener cleanup until the reconnect grace window expires.
- Daemon restart still destroys the forward.
```

Update `crates/remote-exec-daemon-cpp/README.md` with the same behavior boundary.

Update `skills/using-remote-exec-mcp/SKILL.md` so forwarding guidance does not promise immediate cleanup after an ungraceful broker disappearance.

Update the older tunnel design doc so it does not remain the apparent live contract for transport-drop cleanup. Add a note pointing to the reconnect design or revise the cleanup section to reflect resumed listen-side sessions.

- [ ] **Step 2: Run focused verification on docs-adjacent behavior**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: PASS.

- [ ] **Step 3: Run the full quality gate**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add README.md crates/remote-exec-daemon-cpp/README.md skills/using-remote-exec-mcp/SKILL.md docs/superpowers/specs/2026-05-06-port-forward-upgrade-tunnel-design.md
git commit -m "docs: describe reconnectable port forward behavior"
```

- [ ] **Step 5: Tag the implementation checkpoint**

```bash
git status --short
```

Expected: clean working tree, or only intentional uncommitted follow-up notes.

---

## Self-Review

Spec coverage:

- Session control frames and version bump are covered by Task 1.
- Rust listen-side resumable session retention is covered by Task 2.
- Broker retry, resume, and connect-side recreation behavior is covered by Task 3.
- Live daemon transport-drop behavior and grace-window cleanup implications are covered by Task 4.
- C++ daemon parity is covered by Task 5.
- Documentation and full verification are covered by Task 6.

Placeholder scan:

- No `TODO`, `TBD`, or “implement later” placeholders remain.
- Each task names exact files and explicit verification commands.
- All new helpers and frame names introduced in later tasks are defined in earlier task text.

Type consistency:

- Public capability requirement is consistently `port_forward_protocol_version >= 3`.
- Control frame names are consistently `SessionOpen`, `SessionReady`, `SessionResume`, and `SessionResumed`.
- The public forward status remains `open`/`closed`/`failed` throughout the plan.
