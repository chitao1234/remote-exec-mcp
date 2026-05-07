# Port Forward Both-Side Reconnect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Make `forward_ports` recover from retryable transport loss on either forwarding side while preserving the current guarantee level: future listen-side traffic survives, but active TCP streams and UDP per-peer connector state may still be lost.

**Architecture:** Keep the existing v3 listen-side resumable session protocol and merge reconnect policy at the broker bridge layer. Both tunnel roles become recoverable under one retry policy, but recovery action remains role-specific: listen-side recovery resumes an existing session and reopens the connect tunnel, while connect-side recovery reopens only a fresh connect tunnel and resets ephemeral bridge state.

**Tech Stack:** Rust 2024, Tokio async I/O, MCP broker integration tests, repo e2e multi-target tests, C++11 daemon transport/session tests, Make-based C++ host verification.

---

## File Structure

- `crates/remote-exec-broker/src/port_forward/events.rs`: introduce role-aware recovery control instead of listen-only reconnect signaling.
- `crates/remote-exec-broker/src/port_forward/tunnel.rs`: share retryable transport-loss classification across both tunnel roles while keeping explicit tunnel `Error` frames terminal.
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`: recover connect-side transport drops by reopening only the connect tunnel and resetting stream-pairing generation state.
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`: recover connect-side transport drops by reopening only the connect tunnel and resetting peer/connector generation state.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs`: add broker integration coverage for connect-side recovery against the stub daemon reconnect tunnel.
- `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`: add real C++ daemon broker tests for connect-side recovery.
- `tests/e2e/multi_target.rs`: add direction-reversed connect-side reconnect coverage using the existing live daemons.
- `tests/e2e/support/mod.rs`: only modify if existing drop helpers cannot support the new e2e direction-reversed cases.
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`: extend retained-session tests or comments only if needed to keep C++ reconnect expectations explicit and aligned.
- `README.md`: update top-level reconnect wording to say transient transport loss on either side may be recovered with unchanged guarantees.
- `crates/remote-exec-daemon-cpp/README.md`: update C++ daemon reconnect wording to match the broker-visible contract.
- `skills/using-remote-exec-mcp/SKILL.md`: update operator guidance for both-side reconnect wording.

---

### Task 1: Generalize Broker Recovery Control to Either Tunnel Role

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/events.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_does_not_retry_stream_error_frames -- --exact`

**Testing approach:** characterization/integration test
Reason: the existing broker integration suite already verifies “retry transport, do not retry explicit tunnel error” semantics, so this task should preserve that behavior while broadening the transport classifier.

- [ ] **Step 1: Capture the current explicit-error behavior at the broker boundary**

Open `crates/remote-exec-broker/tests/mcp_forward_ports.rs` and keep
`forward_ports_does_not_retry_stream_error_frames` as the characterization
seam. The concrete assertion that must remain true after the refactor is:

```rust
let failed = wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(10)).await;
let error = failed["last_error"].as_str().unwrap_or_default();
assert!(error.contains("connecting tcp forward destination"));
```

- [ ] **Step 2: Run the focused verification baseline**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_does_not_retry_stream_error_frames -- --exact`
Expected: PASS before changes, proving the terminal-error seam is already covered.

- [ ] **Step 3: Implement role-aware recovery control and shared transport classification**

Update `crates/remote-exec-broker/src/port_forward/events.rs` to model the failed tunnel role explicitly:

```rust
pub(super) enum TunnelRole {
    Listen,
    Connect,
}

pub(super) enum ForwardLoopControl {
    Cancelled,
    RecoverTunnel(TunnelRole),
}

pub(super) fn classify_transport_failure(
    err: anyhow::Error,
    context: &'static str,
    role: TunnelRole,
) -> anyhow::Result<ForwardLoopControl> {
    let err = err.context(context);
    if is_retryable_transport_error(&err) {
        Ok(ForwardLoopControl::RecoverTunnel(role))
    } else {
        Err(err)
    }
}
```

Update `crates/remote-exec-broker/src/port_forward/tunnel.rs` to remove the listen-only naming and keep one transport classifier for both sides:

```rust
pub(super) fn classify_recoverable_tunnel_event(result: anyhow::Result<Frame>) -> ForwardSideEvent {
    match result {
        Ok(frame) if frame.frame_type == FrameType::Error => {
            ForwardSideEvent::TerminalTunnelError(decode_tunnel_error_frame(&frame))
        }
        Ok(frame) => ForwardSideEvent::Frame(frame),
        Err(err) if is_retryable_transport_error(&err) => {
            ForwardSideEvent::RetryableTransportLoss
        }
        Err(err) => ForwardSideEvent::TerminalTransportError(err),
    }
}

pub(super) fn is_retryable_transport_error(err: &anyhow::Error) -> bool {
    // move the current listen-side transport logic here unchanged
}
```

Keep the terminal classifier for explicit `Error` frames unchanged in outcome:

```rust
pub(super) fn classify_terminal_tunnel_event(result: anyhow::Result<Frame>) -> ForwardSideEvent {
    match result {
        Ok(frame) if frame.frame_type == FrameType::Error => {
            ForwardSideEvent::TerminalTunnelError(decode_tunnel_error_frame(&frame))
        }
        Ok(frame) => ForwardSideEvent::Frame(frame),
        Err(err) => ForwardSideEvent::TerminalTransportError(err),
    }
}
```

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_does_not_retry_stream_error_frames -- --exact`
Expected: PASS. Explicit daemon `Error` frames still fail immediately instead of entering reconnect.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/events.rs crates/remote-exec-broker/src/port_forward/tunnel.rs
git commit -m "refactor: generalize port tunnel recovery roles"
```

---

### Task 2: Recover Connect-Side TCP Tunnel Loss Without Failing the Forward

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_forward_open_after_connect_tunnel_drop -- --exact`

**Testing approach:** TDD
Reason: the broker stub daemon already supports reconnectable port tunnels, so a focused integration test can drive the bridge logic before broader e2e coverage.

- [ ] **Step 1: Add a failing broker integration test for connect-side TCP recovery**

Add this test in `crates/remote-exec-broker/tests/mcp_forward_ports.rs` next to the listen-side reconnect coverage:

```rust
#[tokio::test]
async fn forward_ports_keeps_forward_open_after_connect_tunnel_drop() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match echo_listener.accept().await {
                Ok(value) => value,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
            });
        }
    });

    let open = fixture.open_remote_tcp_forward_from_local(&echo_addr.to_string()).await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    support::stub_daemon::drop_port_tunnels(&fixture.stub_state).await;

    let entry = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(entry["status"], "open");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    stream.write_all(b"after").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, b"after");
}
```

Use or add a helper in `crates/remote-exec-broker/tests/support/fixture.rs`:

```rust
async fn open_remote_tcp_forward_from_local(&self, connect_endpoint: &str) -> ToolResult {
    self.call_tool(
        "forward_ports",
        serde_json::json!({
            "action": "open",
            "listen_side": "local",
            "connect_side": "builder-a",
            "forwards": [{
                "listen_endpoint": "127.0.0.1:0",
                "connect_endpoint": connect_endpoint,
                "protocol": "tcp"
            }]
        }),
    )
    .await
}
```

- [ ] **Step 2: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_forward_open_after_connect_tunnel_drop -- --exact`
Expected: FAIL because connect-side `recv()` currently returns `reading tcp connect tunnel` and marks the forward failed.

- [ ] **Step 3: Implement connect-side TCP recovery**

Update `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs` so the outer loop handles both recovery roles:

```rust
loop {
    match run_tcp_forward_epoch(&runtime, listen_tunnel.clone(), connect_tunnel.clone()).await? {
        ForwardLoopControl::Cancelled => return Ok(()),
        ForwardLoopControl::RecoverTunnel(TunnelRole::Listen) => {
            let Some(resumed_tunnel) =
                reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone()).await?
            else {
                return Ok(());
            };
            listen_tunnel = resumed_tunnel;
            connect_tunnel = reopen_connect_tunnel(&runtime, "after listen-side reconnect").await?;
        }
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
            connect_tunnel = reopen_connect_tunnel(&runtime, "after connect-side reconnect").await?;
        }
    }
}
```

Add a small helper in the same file to keep messages stable:

```rust
async fn reopen_connect_tunnel(
    runtime: &ForwardRuntime,
    reason: &str,
) -> anyhow::Result<Arc<PortTunnel>> {
    open_connect_tunnel(&runtime.connect_side)
        .await
        .with_context(|| format!("reopening port tunnel to `{}` {reason}", runtime.connect_side.name()))
}
```

Inside `run_tcp_forward_epoch`, treat connect transport loss as recoverable and let the epoch boundary reset all ephemeral state:

```rust
frame = connect_tunnel.recv() => {
    let frame = match classify_recoverable_tunnel_event(frame) {
        ForwardSideEvent::Frame(frame) => frame,
        ForwardSideEvent::RetryableTransportLoss => {
            return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect));
        }
        ForwardSideEvent::TerminalTransportError(err) => {
            return Err(err).context("reading tcp connect tunnel");
        }
        ForwardSideEvent::TerminalTunnelError(meta) => {
            return Err(format_terminal_tunnel_error(&meta))
                .context("connecting tcp forward destination");
        }
    };
    // existing frame handling stays in place
}
```

For the listen-side send paths that already classify retryable transport failure, switch them to the new generic helper:

```rust
return classify_transport_failure(err, "relaying tcp data to listen tunnel", TunnelRole::Listen);
```

The per-epoch locals already provide the needed reset semantics:

```rust
let mut listen_to_connect = HashMap::<u32, u32>::new();
let mut connect_streams = HashMap::<u32, TcpConnectStream>::new();
let mut pending_budget = PendingTcpBudget::default();
```

Do not attempt to preserve those maps across a connect-side generation break.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_forward_open_after_connect_tunnel_drop -- --exact`
Expected: PASS. The public forward remains `open`, and a new TCP connection succeeds after the connect tunnel is reopened.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "feat: recover tcp forwards after connect tunnel loss"
```

---

### Task 3: Recover Connect-Side UDP Tunnel Loss Without Failing the Forward

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_udp_forward_open_after_connect_tunnel_drop -- --exact`

**Testing approach:** TDD
Reason: UDP connector maps are explicitly ephemeral, so a focused broker integration test is the cleanest way to lock down the intended reconnect semantics.

- [ ] **Step 1: Add a failing broker integration test for connect-side UDP recovery**

Add this test in `crates/remote-exec-broker/tests/mcp_forward_ports.rs`:

```rust
#[tokio::test]
async fn forward_ports_keeps_udp_forward_open_after_connect_tunnel_drop() {
    let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    let echo_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_socket.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let mut buf = [0u8; 1024];
            let (read, peer) = match echo_socket.recv_from(&mut buf).await {
                Ok(value) => value,
                Err(_) => return,
            };
            if echo_socket.send_to(&buf[..read], peer).await.is_err() {
                return;
            }
        }
    });

    let open = fixture.open_remote_udp_forward_from_local(&echo_addr.to_string()).await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    support::stub_daemon::drop_port_tunnels(&fixture.stub_state).await;

    let entry = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(entry["status"], "open");

    let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    client.send_to(b"after", &listen_endpoint).await.unwrap();
    let mut buf = [0u8; 16];
    let (read, _) = tokio::time::timeout(Duration::from_secs(5), client.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..read], b"after");
}
```

- [ ] **Step 2: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_udp_forward_open_after_connect_tunnel_drop -- --exact`
Expected: FAIL because connect-side `recv()` currently returns `reading udp connect tunnel`.

- [ ] **Step 3: Implement connect-side UDP recovery**

Update `crates/remote-exec-broker/src/port_forward/udp_bridge.rs` so the outer loop mirrors the TCP recovery shape:

```rust
loop {
    match run_udp_forward_epoch(&runtime, listen_tunnel.clone(), connect_tunnel.clone()).await? {
        ForwardLoopControl::Cancelled => return Ok(()),
        ForwardLoopControl::RecoverTunnel(TunnelRole::Listen) => {
            let Some(resumed_tunnel) =
                reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone()).await?
            else {
                return Ok(());
            };
            listen_tunnel = resumed_tunnel;
            connect_tunnel = reopen_connect_tunnel(&runtime, "after listen-side reconnect").await?;
        }
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
            connect_tunnel = reopen_connect_tunnel(&runtime, "after connect-side reconnect").await?;
        }
    }
}
```

Reclassify connect-side transport loss as recoverable:

```rust
frame = connect_tunnel.recv() => {
    let frame = match classify_recoverable_tunnel_event(frame) {
        ForwardSideEvent::Frame(frame) => frame,
        ForwardSideEvent::RetryableTransportLoss => {
            return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect));
        }
        ForwardSideEvent::TerminalTransportError(err) => {
            return Err(err).context("reading udp connect tunnel");
        }
        ForwardSideEvent::TerminalTunnelError(meta) => {
            return Err(format_terminal_tunnel_error(&meta))
                .context("connect-side udp tunnel error");
        }
    };
    // existing frame handling
}
```

Keep connector reset implicit by epoch scoping:

```rust
let connector_by_peer: Arc<Mutex<HashMap<String, UdpPeerConnector>>> =
    Arc::new(Mutex::new(HashMap::new()));
let peer_by_connector: Arc<Mutex<HashMap<u32, String>>> =
    Arc::new(Mutex::new(HashMap::new()));
```

That reset is the contract: peer mappings are not preserved across reconnect.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_udp_forward_open_after_connect_tunnel_drop -- --exact`
Expected: PASS. A later datagram recreates connector state and relays successfully.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/udp_bridge.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "feat: recover udp forwards after connect tunnel loss"
```

---

### Task 4: Add End-to-End Both-Side Reconnect Coverage and Real C++ Broker Coverage

**Files:**
- Modify: `tests/e2e/multi_target.rs`
- Modify: `tests/e2e/support/mod.rs` only if required
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Modify: `crates/remote-exec-broker/tests/support/fixture.rs` only if the broker integration helper should grow a reversed-direction open helper shared by multiple tests
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp` only if C++ retained-session verification needs clearer alignment
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp cpp_forward_ports_reconnect_after_connect_tunnel_drop -- --exact`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture`, `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** characterization/integration test
Reason: the broker and e2e fixtures already provide live-daemon and real-C++-daemon seams, so the main value here is proving the broker contract across concrete runtime topologies.

- [ ] **Step 1: Add failing real-C++ and e2e connect-side recovery tests**

Add a real C++ daemon broker test in `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs` that reverses direction so the remote side is the connect side:

```rust
#[tokio::test]
async fn cpp_forward_ports_reconnect_after_connect_tunnel_drop() {
    let fixture = CppDaemonBrokerFixture::spawn().await;
    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match echo_listener.accept().await {
                Ok(value) => value,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                stream.read_to_end(&mut buf).await.unwrap();
                stream.write_all(&buf).await.unwrap();
            });
        }
    });

    let open = fixture.open_tcp_forward_local_to_cpp(&echo_addr.to_string()).await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    fixture.drop_port_tunnels().await;

    fixture.wait_for_forward_status(&forward_id, "open", Duration::from_secs(5)).await;

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    stream.write_all(b"after").await.unwrap();
    stream.shutdown().await.unwrap();
    let mut echoed = Vec::new();
    stream.read_to_end(&mut echoed).await.unwrap();
    assert_eq!(echoed, b"after");
}
```

Add e2e direction-reversed tests in `tests/e2e/multi_target.rs`:

```rust
#[tokio::test]
async fn forward_ports_reconnect_after_connect_side_tunnel_drop_and_accept_new_tcp_connections() {
    let cluster = support::spawn_cluster().await;
    let echo_addr = support::spawn_tcp_echo().await;

    let open = cluster
        .broker
        .open_tcp_forward("local", "builder-a", "127.0.0.1:0", &echo_addr.to_string())
        .await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    support::wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "open",
        Duration::from_secs(5),
    )
    .await
    .expect("forward should stay open after connect-side reconnect");

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    stream.write_all(b"after").await.unwrap();
    let mut echoed = [0u8; 5];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"after");
}

#[tokio::test]
async fn forward_ports_reconnect_after_connect_side_tunnel_drop_and_relays_future_udp_datagrams() {
    let cluster = support::spawn_cluster().await;
    let udp_echo = support::spawn_udp_echo().await;

    let open = cluster
        .broker
        .open_udp_forward("local", "builder-a", "127.0.0.1:0", &udp_echo.to_string())
        .await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.drop_port_tunnels().await;

    support::wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "open",
        Duration::from_secs(5),
    )
    .await
    .expect("forward should stay open after connect-side reconnect");

    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender.send_to(b"after", &listen_endpoint).await.unwrap();
    let mut buf = [0u8; 16];
    let (read, _) = tokio::time::timeout(Duration::from_secs(5), sender.recv_from(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..read], b"after");
}
```

If `BrokerFixture` in `tests/e2e/support/mod.rs` needs helpers for reversed direction, add or reuse:

```rust
pub async fn open_tcp_forward(
    &self,
    listen_side: &str,
    connect_side: &str,
    listen_endpoint: &str,
    connect_endpoint: &str,
) -> ToolResult
```

and reuse it for both old and new tests instead of duplicating JSON setup.

- [ ] **Step 2: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp cpp_forward_ports_reconnect_after_connect_tunnel_drop -- --exact`
Expected: FAIL before the broker recovery change is fully exercised against the real C++ daemon.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: FAIL in the new reversed-direction reconnect cases before all helper/bridge changes are complete.

- [ ] **Step 3: Align fixture helpers and C++ verification where needed**

If the e2e support already handles the reversed direction, keep the fixture change minimal. Otherwise, refactor only enough to avoid JSON duplication:

```rust
pub async fn open_udp_forward(
    &self,
    listen_side: &str,
    connect_side: &str,
    listen_endpoint: &str,
    connect_endpoint: &str,
) -> ToolResult {
    self.call_tool(
        "forward_ports",
        serde_json::json!({
            "action": "open",
            "listen_side": listen_side,
            "connect_side": connect_side,
            "forwards": [{
                "listen_endpoint": listen_endpoint,
                "connect_endpoint": connect_endpoint,
                "protocol": "udp"
            }]
        }),
    )
    .await
}
```

Only touch `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp` if the retained-session assertions need wording or a tiny helper cleanup to keep the both-side reconnect contract explicit. Do not attempt to simulate broker orchestration there.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp cpp_forward_ports_reconnect_after_connect_tunnel_drop -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS, including both listen-side and connect-side reconnect cases.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS. Existing C++ retained-session streaming behavior remains intact.

- [ ] **Step 5: Commit**

```bash
git add tests/e2e/multi_target.rs tests/e2e/support/mod.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "test: cover both-side port forward reconnect"
```

---

### Task 5: Update Documentation to Match Both-Side Recovery Semantics

**Files:**
- Modify: `README.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: manual diff review plus targeted text search

**Testing approach:** existing tests + targeted verification
Reason: the behavior change is wording-level in docs, but it needs to be aligned across operator docs and skill guidance in the same change.

- [ ] **Step 1: Update the docs wording**

Revise the reconnect paragraphs so they say transient broker-daemon transport loss on either side may be recovered, without strengthening guarantees.

Update `README.md` text around the existing reconnect section to the equivalent of:

```md
- `forward_ports` survives transient broker-daemon transport disconnects when the daemon stays alive, including loss of either the listen-side or connect-side upgraded tunnel. The daemon retains the forward itself plus future TCP accepts or future UDP datagrams on the listen side, but active TCP streams and UDP per-peer connector state are not preserved across reconnect.
```

Update `crates/remote-exec-daemon-cpp/README.md` to the equivalent of:

```md
Live forwarded sockets are reconnect-aware in-memory daemon state. When only the broker-daemon transport drops and the daemon stays alive, the broker may recover the forward after losing either upgraded tunnel side. The daemon still preserves only the forward itself plus future TCP accepts or future UDP datagrams on the listen side. Active TCP streams and UDP per-peer connector state are not preserved across reconnect.
```

Update `skills/using-remote-exec-mcp/SKILL.md` wording wherever it still implies reconnect is listen-side-only.

- [ ] **Step 2: Run the focused verification**

Run: `rg -n "listen-side|future TCP accepts|UDP per-peer|reconnect" README.md crates/remote-exec-daemon-cpp/README.md skills/using-remote-exec-mcp/SKILL.md`
Expected: the wording consistently says either-side transport loss is recoverable and still states the current limits.

- [ ] **Step 3: Run the main behavior verification before closing out**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add README.md crates/remote-exec-daemon-cpp/README.md skills/using-remote-exec-mcp/SKILL.md
git commit -m "docs: describe both-side port forward reconnect"
```
