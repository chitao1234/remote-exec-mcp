# Port Forward Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Refactor the internal `forward_ports` implementation so forward-session ownership, teardown semantics, error classification, buffering policy, and capability reporting are explicit and maintainable while the public MCP contract stays stable.

**Architecture:** Keep the existing upgrade tunnel and resumable listen-side session protocol, but split the subsystem into public broker state, typed broker bridge logic, host-owned side-session lifecycle, and tunnel transport/codec concerns. Land typed event/error seams and terminal teardown semantics first, then add bounded buffering and truthful capability reporting, and finally split the broker and C++ monoliths into focused modules without changing public behavior.

**Tech Stack:** Rust 2024, Tokio, reqwest upgrades, Hyper/Axum, serde, C++11/XP-compatible sockets and threads, cargo test, cargo fmt, cargo clippy, Make-based C++ host tests

---

## File Structure

- `docs/superpowers/specs/2026-05-07-port-forward-refactor-design.md`: approved refactor spec to keep aligned with implementation sequencing.
- `crates/remote-exec-proto/src/public.rs`: additive public `list_targets` capability field for port-forward protocol version.
- `crates/remote-exec-proto/src/port_tunnel.rs`: tunnel frame definitions; may gain typed helpers or comments if needed while preserving the current wire contract.
- `crates/remote-exec-proto/src/rpc.rs`: existing internal target-info capability field already consumed by broker routing.
- `crates/remote-exec-broker/src/port_forward.rs`: current monolith to be split into focused modules after behavior seams are introduced.
- `crates/remote-exec-broker/src/tools/port_forward.rs`: public tool entrypoints; may stay thin with path updates after module split.
- `crates/remote-exec-broker/src/tools/targets.rs`: `list_targets` structured/text output updates for protocol version reporting.
- `crates/remote-exec-broker/src/state.rs`: forwarding-side validation remains the broker authority for remote capability requirements.
- `crates/remote-exec-broker/src/daemon_client.rs`: keep transport upgrade behavior, but align any error mapping to typed broker-side classification.
- `crates/remote-exec-host/src/port_forward.rs`: current owner of session retention, listener/bind lifecycle, and tunnel read/write loops; first functional refactor target.
- `crates/remote-exec-host/src/state.rs`: local target capability shaping already sets protocol version 3 and will stay the truth source for Rust local/daemon capability reporting.
- `crates/remote-exec-daemon/src/port_forward.rs`: daemon upgrade endpoint wrapper around the shared host runtime.
- `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`: C++ monolith to be behaviorally aligned and then structurally split.
- `crates/remote-exec-daemon-cpp/include/port_tunnel.h`: declarations that will track any C++ file split.
- `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`: C++ target-info capability shaping for protocol-version reporting.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs`: broker-local TCP/UDP forwarding regression coverage.
- `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`: broker-to-real-C++-daemon coverage.
- `crates/remote-exec-broker/tests/mcp_assets.rs`: `list_targets` formatting and structured-content tests.
- `crates/remote-exec-broker/tests/support/stub_daemon.rs`: stub daemon target-info and tunnel error behavior used by broker tests.
- `crates/remote-exec-daemon/tests/port_forward_rpc.rs`: Rust daemon tunnel upgrade and cleanup coverage.
- `tests/e2e/multi_target.rs`: real broker-plus-daemon reconnect/failure/cleanup coverage.
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`: C++ tunnel/session behavior coverage.
- `README.md`: public target-reporting and any user-visible forwarding notes.
- `skills/using-remote-exec-mcp/SKILL.md`: skill guidance if it mentions target capability interpretation.

---

### Task 1: Introduce typed broker-side tunnel error and event seams without changing forwarding behavior

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** `characterization/integration test`
Reason: the current forward behavior is already covered through broker integration tests. This slice should preserve behavior while replacing string-shaped internal policy decisions with typed seams, so focused broker tests are the right guardrail.

- [ ] **Step 1: Add broker tests that pin terminal vs retryable tunnel error classification through the public `forward_ports` surface**

```rust
// crates/remote-exec-broker/tests/mcp_forward_ports.rs
#[tokio::test]
async fn forward_ports_marks_forward_failed_after_terminal_resume_error() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    support::stub_daemon::set_port_forward_support(&mut fixture.stub_state().await, true, 3);
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state().await).await;
    support::stub_daemon::set_port_tunnel_resume_error(
        &fixture.stub_state().await,
        "port_tunnel_resume_expired",
        "port tunnel resume expired",
    )
    .await;

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state().await).await;

    let failed = wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(5)).await;
    let last_error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        last_error.contains("port_tunnel_resume_expired")
            || last_error.contains("port tunnel resume expired"),
        "unexpected error: {last_error}"
    );
}

#[tokio::test]
async fn forward_ports_keeps_forward_open_after_retryable_listen_transport_loss() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    support::stub_daemon::set_port_forward_support(&mut fixture.stub_state().await, true, 3);
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state().await).await;

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "builder-a",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state().await).await;

    let open_entry = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(open_entry["status"], "open");
}
```

```rust
// crates/remote-exec-broker/tests/support/stub_daemon.rs
// Reuse the existing tunnel-control helpers and extend them only if the test
// fixture needs direct accessors:
pub(crate) async fn enable_reconnectable_port_tunnel(state: &StubDaemonState) {
    state.port_tunnel_control.lock().await.enabled = true;
}

pub(crate) async fn force_close_port_tunnel_transport(state: &StubDaemonState) {
    let active_transports = {
        let mut control = state.port_tunnel_control.lock().await;
        std::mem::take(&mut control.active_transports)
    };
    for transport in active_transports {
        transport.cancel();
    }
}

pub(crate) async fn set_port_tunnel_resume_error(
    state: &StubDaemonState,
    code: &str,
    message: &str,
) {
    state.port_tunnel_control.lock().await.resume_behavior = ResumeBehavior::SendError {
        code: code.to_string(),
        message: message.to_string(),
    };
}
```

- [ ] **Step 2: Run the focused verification to capture the current red/green baseline**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS before the refactor if the new characterization tests are written against existing behavior, or FAIL only where broker policy is still message-driven and needs typed handling.

- [ ] **Step 3: Add typed internal tunnel error/event classification inside the broker runtime**

```rust
// crates/remote-exec-broker/src/port_forward.rs
enum ForwardSideEvent {
    Frame(Frame),
    RetryableTransportLoss,
    TerminalTransportError(anyhow::Error),
    TerminalTunnelError {
        code: Option<String>,
        message: String,
        fatal: bool,
        stream_id: u32,
    },
}

enum ForwardSideErrorKind {
    RetryableTransportLoss,
    TerminalProtocol,
    TerminalDaemonCode(String),
}

fn decode_tunnel_error_frame(frame: &Frame) -> ForwardSideEvent {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&frame.meta) else {
        return ForwardSideEvent::TerminalTunnelError {
            code: None,
            message: format!("port tunnel returned error on stream {}", frame.stream_id),
            fatal: true,
            stream_id: frame.stream_id,
        };
    };
    let code = value.get("code").and_then(|v| v.as_str()).map(ToOwned::to_owned);
    let message = value
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("port tunnel error")
        .to_string();
    let fatal = value.get("fatal").and_then(|v| v.as_bool()).unwrap_or(false);
    ForwardSideEvent::TerminalTunnelError {
        code,
        message,
        fatal,
        stream_id: frame.stream_id,
    }
}

fn classify_recv_result(result: anyhow::Result<Frame>) -> ForwardSideEvent {
    match result {
        Ok(frame) if frame.frame_type == FrameType::Error => decode_tunnel_error_frame(&frame),
        Ok(frame) => ForwardSideEvent::Frame(frame),
        Err(err) if is_retryable_listen_transport_error(&err) => ForwardSideEvent::RetryableTransportLoss,
        Err(err) => ForwardSideEvent::TerminalTransportError(err),
    }
}
```

```rust
fn format_terminal_tunnel_error(
    code: Option<&str>,
    message: &str,
    stream_id: u32,
) -> anyhow::Error {
    match code {
        Some(code) => anyhow::anyhow!("{code}: {message}"),
        None if !message.is_empty() => anyhow::anyhow!("{message}"),
        None => anyhow::anyhow!("port tunnel returned error on stream {stream_id}"),
    }
}
```

- [ ] **Step 4: Re-run the broker forwarding suite**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS with unchanged public behavior and no reconnect decisions made from raw message matching except in legacy compatibility shims removed in this slice.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward.rs \
  crates/remote-exec-broker/src/daemon_client.rs \
  crates/remote-exec-broker/tests/mcp_forward_ports.rs \
  crates/remote-exec-broker/tests/support/stub_daemon.rs \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "refactor: type broker port tunnel errors"
```

---

### Task 2: Make host-side close modes explicit and tear down terminal failures immediately

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Modify: `tests/e2e/multi_target.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** `characterization/integration test`
Reason: the behavior change is session-lifecycle sensitive and already exercised best through daemon RPC and real broker-plus-daemon end-to-end tests rather than isolated unit seams.

- [ ] **Step 1: Add failing daemon and e2e tests that distinguish retryable detach from terminal teardown**

```rust
// crates/remote-exec-daemon/tests/port_forward_rpc.rs
#[tokio::test]
async fn terminal_session_error_releases_listener_without_waiting_for_resume_timeout() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let mut stream = open_tunnel(fixture.addr).await;

    write_preface(&mut stream).await.unwrap();
    write_frame(&mut stream, &json_frame(FrameType::SessionOpen, 0, serde_json::json!({})))
        .await
        .unwrap();
    let ready = read_frame(&mut stream).await.unwrap();
    assert_eq!(ready.frame_type, FrameType::SessionReady);

    write_frame(
        &mut stream,
        &json_frame(
            FrameType::TcpListen,
            1,
            serde_json::json!({ "endpoint": "127.0.0.1:0" }),
        ),
    )
    .await
    .unwrap();
    let ok = read_frame(&mut stream).await.unwrap();
    let endpoint = endpoint_from_frame(&ok);

    // Inject a terminal invalid frame instead of clean close.
    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: 0,
            meta: Vec::new(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    wait_until_bindable(&endpoint).await;
}
```

```rust
// tests/e2e/multi_target.rs
#[tokio::test]
async fn forward_ports_terminal_failure_does_not_leave_remote_listener_accepting_until_timeout() {
    let mut cluster = support::spawn_cluster().await;
    let echo_addr = support::spawn_tcp_echo().await;

    let open = cluster
        .broker
        .open_tcp_forward("builder-a", "local", "127.0.0.1:0", &echo_addr.to_string())
        .await;
    let forward_id = open.forward_id();
    let listen_endpoint = open.listen_endpoint();

    cluster.daemon_a.restart().await;

    let failed = support::wait_for_forward_status_timeout(
        &cluster.broker,
        &forward_id,
        "failed",
        Duration::from_secs(5),
    )
    .await
    .expect("forward should fail");
    assert_eq!(failed["status"], "failed");

    // This wait should no longer need to tolerate the resume grace window.
    support::wait_for_daemon_listener_rebind(&listen_endpoint, Duration::from_secs(2)).await;
}
```

- [ ] **Step 2: Run the focused verification**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: FAIL if terminal close still leaves the listener retained by the session timeout path.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: FAIL or hang in the new immediate-teardown assertion until host close modes are made explicit.

- [ ] **Step 3: Add explicit close-mode handling in the shared host runtime**

```rust
// crates/remote-exec-host/src/port_forward.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionCloseMode {
    GracefulClose,
    RetryableDetach,
    TerminalFailure,
}

async fn close_attached_session(tunnel: &Arc<TunnelState>, mode: SessionCloseMode) {
    let Some(session) = tunnel.attached_session.lock().await.take() else {
        return;
    };

    if let Some(attachment) = session.attachment.lock().await.take() {
        attachment.cancel.cancel();
        for (_, cancel) in attachment.stream_cancels.lock().await.drain() {
            cancel.cancel();
        }
        attachment.tcp_writers.lock().await.clear();
    }

    match mode {
        SessionCloseMode::RetryableDetach => {
            *session.resume_deadline.lock().await = Some(Instant::now() + RESUME_TIMEOUT);
            schedule_session_expiry(tunnel.state.port_forward_sessions.clone(), session);
        }
        SessionCloseMode::GracefulClose | SessionCloseMode::TerminalFailure => {
            *session.resume_deadline.lock().await = None;
            tunnel.state.port_forward_sessions.remove(&session.id).await;
            session.close_retained_resources().await;
            session.root_cancel.cancel();
        }
    }
}

pub async fn serve_tunnel<S>(state: Arc<AppState>, stream: S) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // ...
    let result = tunnel_read_loop(tunnel.clone(), &mut reader).await;
    let mode = if result.is_ok() {
        SessionCloseMode::RetryableDetach
    } else {
        SessionCloseMode::TerminalFailure
    };
    close_attached_session(&tunnel, mode).await;
    tunnel.cancel.cancel();
    drop(tx);
    writer_task.abort();
    result
}

async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    // When closing the retained listener/bind explicitly, use GracefulClose
    // semantics instead of the timeout-driven detach path.
}
```

```rust
fn close_mode_for_tunnel_result(result: &Result<(), HostRpcError>) -> SessionCloseMode {
    match result {
        Ok(()) => SessionCloseMode::RetryableDetach,
        Err(_) => SessionCloseMode::TerminalFailure,
    }
}

// Use SessionCloseMode::GracefulClose only from explicit listener/bind close
// paths reached through broker-driven public forward shutdown.
```

- [ ] **Step 4: Re-run the focused verification**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS with reconnect tests still green and the new terminal cleanup assertion passing without a long stale-listener grace period.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host/src/port_forward.rs \
  crates/remote-exec-daemon/tests/port_forward_rpc.rs \
  tests/e2e/multi_target.rs \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "fix: make terminal port forward teardown immediate"
```

---

### Task 3: Add bounded TCP pending buffering and explicit UDP connector limits

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** `characterization/integration test`
Reason: buffering and connector limits are observable in broker-facing behavior and should be validated through the same forwarding paths that carry production traffic.

- [ ] **Step 1: Add failing broker tests for bounded pending TCP data and UDP connector cleanup**

```rust
// crates/remote-exec-broker/tests/mcp_forward_ports.rs
#[tokio::test]
async fn forward_ports_fails_pending_tcp_stream_when_connect_side_never_becomes_ready() {
    let fixture = support::spawners::spawn_broker_local_only().await;
    let blackhole = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let blackhole_addr = blackhole.local_addr().unwrap();
    drop(blackhole);

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": blackhole_addr.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    let oversized = vec![7u8; 1024 * 1024];
    let _ = stream.write_all(&oversized).await;

    let failed = wait_for_forward_status(&fixture, &forward_id, "failed", Duration::from_secs(5)).await;
    let error = failed["last_error"].as_str().unwrap_or_default();
    assert!(
        error.contains("pending")
            || error.contains("backpressure")
            || error.contains("connect"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn forward_ports_closes_excess_udp_peer_connectors_cleanly() {
    let fixture = support::spawners::spawn_broker_local_only().await;
    let echo_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_socket.local_addr().unwrap();
    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        loop {
            let (read, peer) = match echo_socket.recv_from(&mut buf).await {
                Ok(value) => value,
                Err(_) => return,
            };
            if echo_socket.send_to(&buf[..read], peer).await.is_err() {
                return;
            }
        }
    });

    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo_addr.to_string(),
                    "protocol": "udp"
                }]
            }),
        )
        .await;
    let forward_id = open.structured_content["forwards"][0]["forward_id"]
        .as_str()
        .unwrap()
        .to_string();
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .unwrap()
        .to_string();

    for _ in 0..300 {
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let _ = peer.send_to(b"peer", &listen_endpoint).await;
    }

    let list = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "list",
                "forward_ids": [forward_id.clone()]
            }),
        )
        .await;
    assert_eq!(list.structured_content["forwards"][0]["status"], "open");

    let close = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "close",
                "forward_ids": [forward_id]
            }),
        )
        .await;
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}
```

- [ ] **Step 2: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: FAIL because pending TCP data is currently buffered in an unbounded `Vec<Frame>`.

- [ ] **Step 3: Implement bounded buffering and connector policy**

```rust
// crates/remote-exec-broker/src/port_forward.rs
const MAX_PENDING_TCP_BYTES_PER_STREAM: usize = 256 * 1024;
const MAX_PENDING_TCP_BYTES_PER_FORWARD: usize = 2 * 1024 * 1024;
const MAX_UDP_CONNECTORS_PER_FORWARD: usize = 256;

struct TcpConnectStream {
    listen_stream_id: u32,
    ready: bool,
    pending_frames: Vec<Frame>,
    pending_bytes: usize,
}

struct PendingTcpBudget {
    total_bytes: usize,
}

fn frame_data_bytes(frame: &Frame) -> usize {
    frame.meta.len() + frame.data.len()
}

async fn queue_or_send_tcp_connect_frame(
    connect_tunnel: &Arc<PortTunnel>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
    connect_stream_id: u32,
    frame: Frame,
) -> anyhow::Result<()> {
    let Some(stream) = connect_streams.get_mut(&connect_stream_id) else {
        return Ok(());
    };
    if stream.ready {
        connect_tunnel.send(frame).await.context("relaying tcp data to connect tunnel")?;
        return Ok(());
    }

    let added = frame_data_bytes(&frame);
    let next_stream = stream.pending_bytes.saturating_add(added);
    let next_total = pending_budget.total_bytes.saturating_add(added);
    anyhow::ensure!(
        next_stream <= MAX_PENDING_TCP_BYTES_PER_STREAM,
        "pending tcp buffer exceeded per-stream limit"
    );
    anyhow::ensure!(
        next_total <= MAX_PENDING_TCP_BYTES_PER_FORWARD,
        "pending tcp buffer exceeded forward limit"
    );

    stream.pending_bytes = next_stream;
    pending_budget.total_bytes = next_total;
    stream.pending_frames.push(frame);
    Ok(())
}
```

```rust
fn release_pending_budget(
    pending_budget: &mut PendingTcpBudget,
    stream: &mut TcpConnectStream,
) {
    pending_budget.total_bytes = pending_budget
        .total_bytes
        .saturating_sub(stream.pending_bytes);
    stream.pending_bytes = 0;
}

async fn udp_connector_stream_id(
    connect_tunnel: &Arc<PortTunnel>,
    connector_by_peer: &Arc<Mutex<HashMap<String, UdpPeerConnector>>>,
    peer_by_connector: &Arc<Mutex<HashMap<u32, String>>>,
    next_connector_stream_id: &mut u32,
    connector_bind_endpoint: &str,
    peer: String,
) -> anyhow::Result<u32> {
    if let Some(existing) = connector_by_peer.lock().await.get_mut(&peer) {
        existing.last_used = Instant::now();
        return Ok(existing.stream_id);
    }

    anyhow::ensure!(
        connector_by_peer.lock().await.len() < MAX_UDP_CONNECTORS_PER_FORWARD,
        "udp connector limit exceeded"
    );

    let stream_id = *next_connector_stream_id;
    *next_connector_stream_id = next_connector_stream_id.checked_add(2).unwrap_or(1);
    connect_tunnel
        .send(Frame {
            frame_type: FrameType::UdpBind,
            flags: 0,
            stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: connector_bind_endpoint.to_string(),
            })?,
            data: Vec::new(),
        })
        .await
        .context("opening udp connector stream")?;
    connector_by_peer.lock().await.insert(
        peer.clone(),
        UdpPeerConnector {
            stream_id,
            last_used: Instant::now(),
        },
    );
    peer_by_connector.lock().await.insert(stream_id, peer);
    Ok(stream_id)
}
```

- [ ] **Step 4: Re-run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS with a deterministic failure mode for excessive pending TCP buffering.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS for existing reconnect and UDP full-duplex coverage.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward.rs \
  crates/remote-exec-broker/tests/mcp_forward_ports.rs \
  tests/e2e/multi_target.rs \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "fix: bound port forward buffering and connector state"
```

---

### Task 4: Expose truthful port-forward protocol capability through `list_targets`

**Files:**
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets`

**Testing approach:** `existing tests + targeted verification`
Reason: this slice is a schema and formatting extension for an existing tool, so public broker tests and doc updates are the relevant proof.

- [ ] **Step 1: Add failing `list_targets` tests for the protocol-version field**

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs
#[tokio::test]
async fn list_targets_reports_port_forward_protocol_version_when_available() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let mut state = fixture.stub_state().await;
    support::stub_daemon::set_port_forward_support(&mut state, true, 3);

    let result = fixture
        .call_tool("list_targets", serde_json::json!({}))
        .await;

    assert_eq!(
        result.structured_content["targets"][0]["daemon_info"]["port_forward_protocol_version"],
        serde_json::json!(3)
    );
    assert!(
        result.text_output.contains("forward_ports=yes")
            && result.text_output.contains("forward_protocol=v3"),
        "unexpected text: {}",
        result.text_output
    );
}
```

- [ ] **Step 2: Run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: FAIL because the public structured/text output does not yet include the protocol version.

- [ ] **Step 3: Add the public field and update formatting/docs**

```rust
// crates/remote-exec-proto/src/public.rs
pub struct ListTargetDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_port_forward: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port_forward_protocol_version: Option<u32>,
}
```

```rust
// crates/remote-exec-broker/src/tools/targets.rs
daemon_info: handle.cached_daemon_info().await.map(|info| ListTargetDaemonInfo {
    daemon_version: info.daemon_version,
    hostname: info.hostname,
    platform: info.platform,
    arch: info.arch,
    supports_pty: info.supports_pty,
    supports_port_forward: info.supports_port_forward,
    port_forward_protocol_version: info.supports_port_forward.then_some(info.port_forward_protocol_version),
});

// Include the version in human output only when forwarding is supported.
format!(
    "- {}: {}/{}, host={}, version={}, pty={}, forward_ports={}, forward_protocol={}",
    // ...
    if info.supports_port_forward { "yes" } else { "no" },
    info.port_forward_protocol_version
        .map(|version| format!("v{version}"))
        .unwrap_or_else(|| "n/a".to_string()),
)
```

```text
# README.md and skills/using-remote-exec-mcp/SKILL.md
Explain that `list_targets` now reports both whether forwarding is supported
and which internal port-forward protocol version the broker sees for that target.

Recommended wording to add:
- "`forward_ports=yes` means the daemon reports port-forward support."
- "`forward_protocol=vN` shows the internal broker-daemon forwarding protocol version cached for that target."
```

- [ ] **Step 4: Re-run the focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/public.rs \
  crates/remote-exec-broker/src/tools/targets.rs \
  crates/remote-exec-broker/tests/mcp_assets.rs \
  crates/remote-exec-broker/tests/support/stub_daemon.rs \
  README.md \
  skills/using-remote-exec-mcp/SKILL.md \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "feat: report port forward protocol version in list targets"
```

---

### Task 5: Split the broker port-forward monolith into focused Rust modules

**Files:**
- Create: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Create: `crates/remote-exec-broker/src/port_forward/store.rs`
- Create: `crates/remote-exec-broker/src/port_forward/side.rs`
- Create: `crates/remote-exec-broker/src/port_forward/events.rs`
- Create: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Create: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Create: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Create: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/tools/port_forward.rs`
- Delete: `crates/remote-exec-broker/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** `existing tests + targeted verification`
Reason: this slice is primarily structural after the earlier behavior changes are locked in. Existing broker and e2e forwarding suites are the right protection against regressions.

- [ ] **Step 1: Create the new broker module layout and move the store/types first**

```rust
// crates/remote-exec-broker/src/port_forward/mod.rs
mod events;
mod side;
mod store;
mod supervisor;
mod tcp_bridge;
mod tunnel;
mod udp_bridge;

pub use side::SideHandle;
pub use store::{
    OpenedForward, PortForwardFilter, PortForwardRecord, PortForwardStore,
    close_all, close_record,
};
pub use supervisor::open_forward;
```

```rust
// store.rs
pub struct PortForwardStore { /* moved from the old monolith */ }
pub struct PortForwardFilter { /* moved from the old monolith */ }
pub struct PortForwardRecord { /* moved from the old monolith */ }
pub struct OpenedForward { /* moved from the old monolith */ }

// side.rs
pub enum SideHandle { /* moved from the old monolith */ }

// tunnel.rs
pub struct PortTunnel { /* moved from the old monolith */ }
```

- [ ] **Step 2: Run the focused verification after the initial move**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS after imports are updated.

- [ ] **Step 3: Move TCP/UDP relay loops and reconnect supervision into dedicated modules**

```rust
// crates/remote-exec-broker/src/port_forward/tcp_bridge.rs
pub(super) async fn run_tcp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> { /* moved */ }

// crates/remote-exec-broker/src/port_forward/udp_bridge.rs
pub(super) async fn run_udp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> { /* moved */ }

// crates/remote-exec-broker/src/port_forward/supervisor.rs
pub async fn open_forward(
    store: PortForwardStore,
    listen_side: SideHandle,
    connect_side: SideHandle,
    spec: &ForwardPortSpec,
) -> anyhow::Result<OpenedForward> { /* moved and re-exported */ }
```

```rust
// events.rs
pub enum ForwardSideEvent { /* moved typed events */ }
pub enum ForwardSideErrorKind { /* moved typed broker error kinds */ }

// tunnel.rs
pub fn classify_recv_result(result: anyhow::Result<Frame>) -> ForwardSideEvent { /* moved */ }
pub fn decode_tunnel_error_frame(frame: &Frame) -> ForwardSideEvent { /* moved */ }
```

- [ ] **Step 4: Re-run the broker forwarding suites**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/lib.rs \
  crates/remote-exec-broker/src/tools/port_forward.rs \
  crates/remote-exec-broker/src/port_forward \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "refactor: split broker port forward runtime"
```

---

### Task 6: Split the shared host runtime port-forward monolith into focused Rust modules

**Files:**
- Create: `crates/remote-exec-host/src/port_forward/mod.rs`
- Create: `crates/remote-exec-host/src/port_forward/session_store.rs`
- Create: `crates/remote-exec-host/src/port_forward/session.rs`
- Create: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Create: `crates/remote-exec-host/src/port_forward/udp.rs`
- Create: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Create: `crates/remote-exec-host/src/port_forward/codec.rs`
- Create: `crates/remote-exec-host/src/port_forward/error.rs`
- Modify: `crates/remote-exec-host/src/lib.rs`
- Delete: `crates/remote-exec-host/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`, `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** `existing tests + targeted verification`
Reason: once session behavior is fixed, this slice is a transport/runtime structure refactor best protected by daemon RPC and broker-facing regression suites.

- [ ] **Step 1: Move codec and error helpers into dedicated modules without changing behavior**

```rust
// crates/remote-exec-host/src/port_forward/mod.rs
mod codec;
mod error;
mod session;
mod session_store;
mod tcp;
mod tunnel;
mod udp;

pub use session_store::TunnelSessionStore;
pub use tunnel::serve_tunnel;
```

```rust
// codec.rs
pub(super) fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, HostRpcError> { /* moved */ }
pub(super) fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, HostRpcError> { /* moved */ }

// error.rs
pub(super) fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError { /* moved */ }
pub(super) enum SessionCloseMode { GracefulClose, RetryableDetach, TerminalFailure }
```

- [ ] **Step 2: Run the focused verification after the first extraction**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

- [ ] **Step 3: Move session store/session lifecycle/TCP/UDP loops into focused files**

```rust
// session_store.rs
pub struct TunnelSessionStore { /* moved */ }

// session.rs
struct SessionState { /* moved */ }
struct AttachmentState { /* moved */ }
async fn close_attached_session(tunnel: &Arc<TunnelState>, mode: SessionCloseMode) { /* moved */ }

// tcp.rs
async fn tunnel_tcp_listen(...) -> Result<(), HostRpcError> { /* moved */ }
async fn tunnel_tcp_connect(...) -> Result<(), HostRpcError> { /* moved */ }
async fn tunnel_tcp_read_loop_session_owned(...) { /* moved */ }

// udp.rs
async fn tunnel_udp_bind(...) -> Result<(), HostRpcError> { /* moved */ }
async fn tunnel_udp_datagram(...) -> Result<(), HostRpcError> { /* moved */ }
async fn tunnel_udp_read_loop_session_owned(...) { /* moved */ }

// tunnel.rs
pub async fn serve_tunnel<S>(...) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{ /* moved */ }
```

```rust
// Keep public exports stable for the rest of the workspace:
pub use session_store::TunnelSessionStore;
pub use tunnel::serve_tunnel;
```

- [ ] **Step 4: Re-run the focused verification**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host/src/lib.rs \
  crates/remote-exec-host/src/port_forward \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "refactor: split host port forward runtime"
```

---

### Task 7: Align and split the C++ port-tunnel implementation

**Files:**
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/port_tunnel.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`, `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`

**Testing approach:** `existing tests + targeted verification`
Reason: this slice is behavior-preserving after Rust semantics are already locked down. Existing C++ host tests and broker-to-real-C++-daemon tests should prove parity.

- [ ] **Step 1: Add or tighten C++ tests for immediate terminal teardown and protocol-version reporting**

```cpp
// crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
static void test_terminal_session_error_releases_listener_immediately() {
    TestServer server = spawn_test_server();
    SOCKET socket = open_port_tunnel(server.address);
    send_all_bytes(socket, port_tunnel_preface(), port_tunnel_preface_size());
    send_tunnel_frame(socket, make_json_frame(PortTunnelFrameType::SessionOpen, 0U, "{}"));
    const PortTunnelFrame ready = read_tunnel_frame(socket);
    assert(ready.type == PortTunnelFrameType::SessionReady);
    send_tunnel_frame(
        socket,
        make_json_frame(PortTunnelFrameType::TcpListen, 1U, "{\"endpoint\":\"127.0.0.1:0\"}")
    );
    const PortTunnelFrame ok = read_tunnel_frame(socket);
    const std::string endpoint = frame_meta_string(ok, "endpoint");
    send_tunnel_frame(socket, make_empty_frame(static_cast<PortTunnelFrameType>(255), 0U));
    close_socket(socket);
    wait_until_bindable(endpoint);
}
```

- [ ] **Step 2: Run the focused verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: FAIL until the C++ close-mode behavior and/or file split is aligned.

- [ ] **Step 3: Extract the C++ port-tunnel concerns into focused translation units**

```cpp
// port_tunnel_session.cpp
// create_session, find_session, attach_session, detach_session, close_session,
// expiry cleanup helpers

// port_tunnel_transport.cpp
// read_frame, send_frame, run transport dispatch loop, upgrade-lifetime close handling

// port_tunnel_tcp.cpp
// tcp_listen, tcp_connect, tcp_accept_loop_transport_owned, tcp_read_loop

// port_tunnel_udp.cpp
// udp_bind, udp_datagram, udp_read_loop_transport_owned

// port_tunnel_error.cpp
// send_error, session close-mode to error mapping, stable code/message helpers
```

```cpp
// Keep port_tunnel.cpp as the small integration point or reduce it to a thin
// coordinator depending on how the existing declarations are easiest to preserve.
```

- [ ] **Step 4: Re-run the focused verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/port_tunnel.h \
  crates/remote-exec-daemon-cpp/src/port_tunnel.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp \
  crates/remote-exec-daemon-cpp/src/server_route_common.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp \
  crates/remote-exec-daemon-cpp/Makefile \
  crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs \
  docs/superpowers/plans/2026-05-07-port-forward-refactor.md
git commit -m "refactor: split cpp port tunnel runtime"
```

---

### Task 8: Run the full quality gate and land the refactor

**Files:**
- Modify: none if all verification passes cleanly
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** `existing tests + targeted verification`
Reason: the refactor spans broker, shared host runtime, Rust daemon, C++ daemon, public proto, docs, and end-to-end forwarding behavior, so the full workspace and C++ gates are required before completion.

- [ ] **Step 1: Run the full Rust workspace test suite**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 2: Run formatting and lint gates**

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 3: Run the full C++ POSIX verification gate**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 4: Commit the final verification or cleanup adjustments if needed**

```bash
git add .
git commit -m "refactor: clean up port forward architecture"
```
