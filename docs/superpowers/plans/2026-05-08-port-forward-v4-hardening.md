# Port Forward v4 Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Implement the approved v4 `forward_ports` hardening design with aligned tunnel protocol version `4`, explicit tunnel lifecycle, visible reconnect state, resource limits, operation timeouts, byte-aware pressure handling, generation-safe stream IDs, and Rust/C++ daemon parity.

**Architecture:** Keep broker-owned public `forward_id` values and broker-mediated data flow. Add v4 protocol/control frames in `remote-exec-proto`, move broker forwarding into a generation-aware supervisor with explicit `PortTunnel` ownership, upgrade the Rust host daemon first, then bring the C++ daemon to the same v4 contract.

**Tech Stack:** Rust 2024, Tokio async I/O, Axum/Hyper HTTP/1.1 upgrades, reqwest upgrades, serde/schemars public schemas, C++11 socket/thread daemon, Make-based C++ checks, broker/daemon/e2e Rust tests.

---

## File Structure

- `crates/remote-exec-proto/src/port_tunnel.rs`: v4 tunnel constants, new control frame types, metadata structs, frame I/O tests, stable error metadata.
- `crates/remote-exec-proto/src/public.rs`: additive public `ForwardPortEntry` fields and supporting enums/structs.
- `crates/remote-exec-proto/src/rpc.rs`: daemon target info additions for forwarding limits when exposed.
- `crates/remote-exec-proto/src/port_forward.rs`: endpoint helpers remain shared; add generation/stream ID helper tests if common logic is placed here.
- `crates/remote-exec-host/src/port_forward/`: Rust host v4 control frames, close modes, retained listener/bind maps, limits, generation checks, byte-aware queue handling.
- `crates/remote-exec-daemon/src/port_forward.rs`: v4 upgrade validation and route wiring for Rust daemon.
- `crates/remote-exec-broker/src/daemon_client.rs`: v4 tunnel upgrade header, upgrade timeout, preface timeout.
- `crates/remote-exec-broker/src/state.rs`: v4 capability gate.
- `crates/remote-exec-broker/src/port_forward/tunnel.rs`: explicit lifecycle-owned `PortTunnel`.
- `crates/remote-exec-broker/src/port_forward/supervisor.rs`: v4 open/close, phase transitions, symmetric reconnect policy.
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`: generation-aware TCP stream pairing, active stream counters, data pressure behavior.
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`: generation-aware UDP peer connectors, datagram drop counters, connect-side recovery.
- `crates/remote-exec-broker/src/port_forward/store.rs`: store public state updates and close/failure behavior.
- `crates/remote-exec-broker/src/port_forward/events.rs`: side role, loop control, reconnect classifications.
- `crates/remote-exec-broker/src/port_forward/limits.rs`: create broker-side limit structs and accounting helpers.
- `crates/remote-exec-broker/src/port_forward/generation.rs`: create generation and stream ID allocation helpers.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs`: broker integration coverage for public fields, v4 gate, reconnect state, limits, timeouts, pressure, generation rotation.
- `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`: C++ daemon parity coverage through real broker path.
- `crates/remote-exec-broker/tests/support/stub_daemon.rs`: v4-aware stub tunnel behavior and fault injection.
- `crates/remote-exec-daemon/tests/port_forward_rpc.rs`: Rust daemon v4 upgrade/control/retention/limit tests.
- `tests/e2e/multi_target.rs`: multi-target Rust daemon v4 reconnect and lifecycle coverage.
- `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`: C++ frame enum additions.
- `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`: C++ frame codec additions.
- `crates/remote-exec-daemon-cpp/src/port_tunnel*.cpp`: C++ v4 control frames, close modes, limits, generation checks.
- `crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp`: C++ frame/control metadata tests.
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`: C++ tunnel lifecycle/reconnect tests.
- `README.md`, `configs/broker.example.toml`, `configs/daemon.example.toml`, `crates/remote-exec-daemon-cpp/README.md`, `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`, `skills/using-remote-exec-mcp/SKILL.md`: v4 docs and operator guidance.

---

### Task 1: Protocol v4 Contract and Public Result Schema

**Files:**
- Modify: `crates/remote-exec-proto/src/port_tunnel.rs`
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify: `cargo test -p remote-exec-proto port_tunnel --lib`

**Testing approach:** TDD
Reason: protocol constants, frame types, and public schema additions are isolated seams and should compile before broker/daemon behavior changes.

- [ ] **Step 1: Add failing v4 protocol tests**

Append these tests to `crates/remote-exec-proto/src/port_tunnel.rs`:

```rust
#[tokio::test]
async fn v4_control_frames_round_trip() {
    let (mut client, mut server) = tokio::io::duplex(4096);
    let writer = tokio::spawn(async move {
        write_frame(
            &mut client,
            &Frame {
                frame_type: FrameType::TunnelOpen,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&TunnelOpenMeta {
                    forward_id: "fwd_test".to_string(),
                    role: TunnelRole::Listen,
                    side: "builder-a".to_string(),
                    generation: 4,
                    protocol: TunnelForwardProtocol::Tcp,
                    resume_session_id: Some("sess_test".to_string()),
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
    });

    let frame = read_frame(&mut server).await.unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(frame.frame_type, FrameType::TunnelOpen);
    assert_eq!(frame.stream_id, 0);
    let meta: TunnelOpenMeta = serde_json::from_slice(&frame.meta).unwrap();
    assert_eq!(meta.forward_id, "fwd_test");
    assert_eq!(meta.role, TunnelRole::Listen);
    assert_eq!(meta.generation, 4);
    assert_eq!(meta.resume_session_id.as_deref(), Some("sess_test"));
}

#[test]
fn tunnel_protocol_version_is_aligned_to_v4() {
    assert_eq!(TUNNEL_PROTOCOL_VERSION, "4");
    assert_eq!(
        TUNNEL_PROTOCOL_VERSION_HEADER,
        "x-remote-exec-port-tunnel-version"
    );
}
```

Append this schema serialization test to `crates/remote-exec-proto/src/public.rs` under a new `#[cfg(test)]` module:

```rust
#[test]
fn forward_port_entry_serializes_additive_v4_state() {
    let entry = ForwardPortEntry {
        forward_id: "fwd_test".to_string(),
        listen_side: "local".to_string(),
        listen_endpoint: "127.0.0.1:10000".to_string(),
        connect_side: "builder-a".to_string(),
        connect_endpoint: "127.0.0.1:10001".to_string(),
        protocol: ForwardPortProtocol::Tcp,
        status: ForwardPortStatus::Open,
        last_error: None,
        phase: ForwardPortPhase::Reconnecting,
        listen_state: ForwardPortSideState {
            side: "local".to_string(),
            role: ForwardPortSideRole::Listen,
            generation: 2,
            health: ForwardPortSideHealth::Ready,
            last_error: None,
        },
        connect_state: ForwardPortSideState {
            side: "builder-a".to_string(),
            role: ForwardPortSideRole::Connect,
            generation: 3,
            health: ForwardPortSideHealth::Reconnecting,
            last_error: Some("transport loss".to_string()),
        },
        active_tcp_streams: 1,
        dropped_tcp_streams: 2,
        dropped_udp_datagrams: 3,
        reconnect_attempts: 4,
        last_reconnect_at: Some("2026-05-08T00:00:00Z".to_string()),
        limits: ForwardPortLimitSummary {
            max_active_tcp_streams: 256,
            max_udp_peers: 256,
            max_pending_tcp_bytes_per_stream: 262144,
            max_pending_tcp_bytes_per_forward: 2097152,
            max_tunnel_queued_bytes: 8388608,
        },
    };

    let value = serde_json::to_value(entry).unwrap();
    assert_eq!(value["phase"], "reconnecting");
    assert_eq!(value["connect_state"]["health"], "reconnecting");
    assert_eq!(value["dropped_tcp_streams"], 2);
    assert_eq!(value["limits"]["max_tunnel_queued_bytes"], 8388608);
}
```

- [ ] **Step 2: Run the failing protocol/schema tests**

Run: `cargo test -p remote-exec-proto port_tunnel --lib`
Expected: FAIL because v4 frame types and metadata structs do not exist yet.

Run: `cargo test -p remote-exec-proto forward_port_entry_serializes_additive_v4_state --lib`
Expected: FAIL because public v4 result fields do not exist yet.

- [ ] **Step 3: Implement v4 protocol types**

In `crates/remote-exec-proto/src/port_tunnel.rs`, set:

```rust
pub const TUNNEL_PROTOCOL_VERSION: &str = "4";
```

Extend `FrameType`:

```rust
pub enum FrameType {
    Error = 1,
    Close = 2,
    SessionOpen = 3,
    SessionReady = 4,
    SessionResume = 5,
    SessionResumed = 6,
    TunnelOpen = 7,
    TunnelReady = 8,
    TunnelClose = 9,
    TcpListen = 10,
    TcpListenOk = 11,
    TcpAccept = 12,
    TcpConnect = 13,
    TcpConnectOk = 14,
    TcpData = 15,
    TcpEof = 16,
    TunnelClosed = 17,
    TunnelHeartbeat = 18,
    TunnelHeartbeatAck = 19,
    ForwardRecovering = 20,
    ForwardRecovered = 21,
    UdpBind = 30,
    UdpBindOk = 31,
    UdpDatagram = 32,
}
```

Update `FrameType::from_u8` with matching numeric arms. Add metadata structs:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelRole {
    Listen,
    Connect,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelForwardProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelOpenMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub side: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
    pub protocol: TunnelForwardProtocol,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelReadyMeta {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_timeout_ms: Option<u64>,
    pub limits: TunnelLimitSummary,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelLimitSummary {
    pub max_active_tcp_streams: u64,
    pub max_udp_peers: u64,
    pub max_queued_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelCloseMeta {
    pub forward_id: String,
    pub generation: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardRecoveringMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub old_generation: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardRecoveredMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub generation: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelErrorMeta {
    pub code: String,
    pub message: String,
    pub fatal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}
```

- [ ] **Step 4: Implement additive public result fields**

In `crates/remote-exec-proto/src/public.rs`, add:

```rust
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortPhase {
    Opening,
    Ready,
    Reconnecting,
    Draining,
    Closing,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortSideRole {
    Listen,
    Connect,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardPortSideHealth {
    Starting,
    Ready,
    Reconnecting,
    Degraded,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ForwardPortSideState {
    pub side: String,
    pub role: ForwardPortSideRole,
    pub generation: u64,
    pub health: ForwardPortSideHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, JsonSchema, PartialEq, Eq)]
pub struct ForwardPortLimitSummary {
    pub max_active_tcp_streams: u64,
    pub max_udp_peers: u64,
    pub max_pending_tcp_bytes_per_stream: u64,
    pub max_pending_tcp_bytes_per_forward: u64,
    pub max_tunnel_queued_bytes: u64,
}
```

Add the fields from the approved spec to `ForwardPortEntry`. Then add an
associated constructor helper so broker code does not repeat defaults:

```rust
impl ForwardPortEntry {
    pub fn new_open(
        forward_id: String,
        listen_side: String,
        listen_endpoint: String,
        connect_side: String,
        connect_endpoint: String,
        protocol: ForwardPortProtocol,
        limits: ForwardPortLimitSummary,
    ) -> Self {
        Self {
            forward_id,
            listen_side: listen_side.clone(),
            listen_endpoint,
            connect_side: connect_side.clone(),
            connect_endpoint,
            protocol,
            status: ForwardPortStatus::Open,
            last_error: None,
            phase: ForwardPortPhase::Ready,
            listen_state: ForwardPortSideState {
                side: listen_side,
                role: ForwardPortSideRole::Listen,
                generation: 1,
                health: ForwardPortSideHealth::Ready,
                last_error: None,
            },
            connect_state: ForwardPortSideState {
                side: connect_side,
                role: ForwardPortSideRole::Connect,
                generation: 1,
                health: ForwardPortSideHealth::Ready,
                last_error: None,
            },
            active_tcp_streams: 0,
            dropped_tcp_streams: 0,
            dropped_udp_datagrams: 0,
            reconnect_attempts: 0,
            last_reconnect_at: None,
            limits,
        }
    }
}
```

- [ ] **Step 5: Run focused verification**

Run: `cargo test -p remote-exec-proto port_tunnel --lib`
Expected: PASS.

Run: `cargo test -p remote-exec-proto forward_port_entry_serializes_additive_v4_state --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-proto/src/port_tunnel.rs crates/remote-exec-proto/src/public.rs crates/remote-exec-proto/src/rpc.rs
git commit -m "feat: define port forward v4 contract"
```

---

### Task 2: Capability Gate and Test Fixtures Move to v4

**Files:**
- Modify: `crates/remote-exec-host/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/state.rs`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets list_targets_reports_port_forward_protocol_version_when_available -- --exact`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_targets_without_tunnel_protocol_version -- --exact`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** TDD
Reason: the broker must reject stale targets before any runtime path is used, and target-info output is a stable compatibility boundary.

- [ ] **Step 1: Update failing broker expectation to v4**

In `crates/remote-exec-broker/tests/mcp_forward_ports.rs`, update
`forward_ports_rejects_targets_without_tunnel_protocol_version` to spawn a
version `3` daemon and assert the v4 message:

```rust
let fixture = support::spawners::spawn_broker_with_stub_port_forward_version(3).await;

let error = fixture.call_tool_error(/* existing forward_ports open input */).await;
assert!(
    error.contains("does not support port forward protocol version 4"),
    "unexpected error: {error}"
);
```

In `crates/remote-exec-broker/tests/mcp_assets.rs`, update list-targets protocol
expectations to `4`.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_targets_without_tunnel_protocol_version -- --exact`
Expected: FAIL while broker still accepts protocol version `3`.

- [ ] **Step 3: Set Rust target-info and broker gate to v4**

In `crates/remote-exec-host/src/lib.rs`, find `target_info_response` and set:

```rust
supports_port_forward: true,
port_forward_protocol_version: 4,
```

In `crates/remote-exec-broker/src/state.rs`, change the forwarding gate:

```rust
anyhow::ensure!(
    info.supports_port_forward && info.port_forward_protocol_version >= 4,
    "target `{name}` does not support port forward protocol version 4"
);
```

Update broker test spawners and stub helpers so v4-capable fixtures pass `4`
instead of `3`:

```rust
set_port_forward_support(&mut stub_state, true, 4);
```

- [ ] **Step 4: Set C++ target-info to v4**

In `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`, update the
target-info JSON field:

```cpp
{"supports_port_forward", true},
{"port_forward_protocol_version", 4}
```

Update `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp` assertions
to expect `4`.

- [ ] **Step 5: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_assets list_targets_reports_port_forward_protocol_version_when_available -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_targets_without_tunnel_protocol_version -- --exact`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-host/src/lib.rs crates/remote-exec-broker/src/state.rs crates/remote-exec-broker/src/tools/targets.rs crates/remote-exec-broker/tests/mcp_assets.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs crates/remote-exec-broker/tests/support/spawners.rs crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-daemon-cpp/src/server_route_common.cpp crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
git commit -m "feat: require port forward protocol v4"
```

---

### Task 3: Rust Daemon v4 Upgrade and Control Frames

**Files:**
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`

**Testing approach:** TDD
Reason: v4 daemon behavior is observable through the private HTTP upgrade route and host tunnel control frames before the broker switches to v4 control.

- [ ] **Step 1: Add failing Rust daemon tests**

In `crates/remote-exec-daemon/tests/port_forward_rpc.rs`, add:

```rust
#[tokio::test]
async fn port_tunnel_requires_v4_header() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let client = reqwest::Client::new();
    let response = client
        .post(fixture.url("/v1/port/tunnel"))
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, remote_exec_proto::port_tunnel::UPGRADE_TOKEN)
        .header(remote_exec_proto::port_tunnel::TUNNEL_PROTOCOL_VERSION_HEADER, "3")
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: remote_exec_proto::rpc::RpcErrorBody = response.json().await.unwrap();
    assert_eq!(body.code, "bad_request");
    assert!(body.message.contains("x-remote-exec-port-tunnel-version: 4"));
}

#[tokio::test]
async fn tunnel_open_ready_and_close_round_trip() {
    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, TunnelCloseMeta, TunnelForwardProtocol, TunnelOpenMeta,
        TunnelReadyMeta, TunnelRole, read_frame, write_frame,
    };

    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let mut stream = fixture.open_port_tunnel_v4().await;
    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelOpenMeta {
                forward_id: "fwd_test".to_string(),
                role: TunnelRole::Listen,
                side: "builder-a".to_string(),
                generation: 1,
                protocol: TunnelForwardProtocol::Tcp,
                resume_session_id: None,
            })
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    let ready = read_frame(&mut stream).await.unwrap();
    assert_eq!(ready.frame_type, FrameType::TunnelReady);
    let ready_meta: TunnelReadyMeta = serde_json::from_slice(&ready.meta).unwrap();
    assert_eq!(ready_meta.generation, 1);
    assert!(ready_meta.session_id.is_some());

    write_frame(
        &mut stream,
        &Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&TunnelCloseMeta {
                forward_id: "fwd_test".to_string(),
                generation: 1,
                reason: "operator_close".to_string(),
            })
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    let closed = read_frame(&mut stream).await.unwrap();
    assert_eq!(closed.frame_type, FrameType::TunnelClosed);
}
```

Add this helper to `crates/remote-exec-daemon/tests/support/fixture.rs` inside
the existing `impl DaemonFixture` block:

```rust
pub async fn open_port_tunnel_v4(&self) -> reqwest::Upgraded {
    let response = reqwest::Client::new()
        .post(self.url("/v1/port/tunnel"))
        .header(reqwest::header::CONNECTION, "Upgrade")
        .header(reqwest::header::UPGRADE, remote_exec_proto::port_tunnel::UPGRADE_TOKEN)
        .header(
            remote_exec_proto::port_tunnel::TUNNEL_PROTOCOL_VERSION_HEADER,
            remote_exec_proto::port_tunnel::TUNNEL_PROTOCOL_VERSION,
        )
        .header(reqwest::header::CONTENT_LENGTH, "0")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SWITCHING_PROTOCOLS);
    let mut stream = response.upgrade().await.unwrap();
    remote_exec_proto::port_tunnel::write_preface(&mut stream)
        .await
        .unwrap();
    stream
}
```

- [ ] **Step 2: Run failing daemon tests**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_requires_v4_header -- --exact`
Expected: FAIL while the daemon error still references version `2`.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc tunnel_open_ready_and_close_round_trip -- --exact`
Expected: FAIL while `TunnelOpen` is not handled.

- [ ] **Step 3: Update Rust daemon upgrade validation**

In `crates/remote-exec-daemon/src/port_forward.rs`, ensure the version header
uses `TUNNEL_PROTOCOL_VERSION`, now `4`, and error text includes the required
header value:

```rust
if !header_eq(
    headers,
    TUNNEL_PROTOCOL_VERSION_HEADER,
    TUNNEL_PROTOCOL_VERSION,
) {
    return Err(bad_upgrade_request(format!(
        "missing `{TUNNEL_PROTOCOL_VERSION_HEADER}: {TUNNEL_PROTOCOL_VERSION}` header"
    )));
}
```

- [ ] **Step 4: Handle v4 control frames in Rust host tunnel**

In `crates/remote-exec-host/src/port_forward/tunnel.rs`, route new frame types:

```rust
FrameType::TunnelOpen => tunnel_open(tunnel, frame).await,
FrameType::TunnelClose => tunnel_close(tunnel, frame).await,
FrameType::TunnelHeartbeat => tunnel
    .send(Frame {
        frame_type: FrameType::TunnelHeartbeatAck,
        flags: 0,
        stream_id: 0,
        meta: frame.meta,
        data: Vec::new(),
    })
    .await,
```

Implement `tunnel_open` so listen role creates or resumes a session and sends
`TunnelReady`. Implement connect role as transport-owned and sends
`TunnelReady` without retained session metadata:

```rust
async fn tunnel_open(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: remote_exec_proto::port_tunnel::TunnelOpenMeta = decode_frame_meta(&frame)?;
    if frame.stream_id != 0 {
        return Err(rpc_error("invalid_port_tunnel", "tunnel open must use stream_id 0"));
    }
    match meta.role {
        remote_exec_proto::port_tunnel::TunnelRole::Listen => {
            open_or_resume_listen_session(tunnel, meta).await
        }
        remote_exec_proto::port_tunnel::TunnelRole::Connect => {
            tunnel
                .send(Frame {
                    frame_type: FrameType::TunnelReady,
                    flags: 0,
                    stream_id: 0,
                    meta: encode_frame_meta(&remote_exec_proto::port_tunnel::TunnelReadyMeta {
                        generation: meta.generation,
                        session_id: None,
                        resume_timeout_ms: None,
                        limits: default_tunnel_limit_summary(),
                    })?,
                    data: Vec::new(),
                })
                .await
        }
    }
}
```

Keep existing `SessionOpen`/`SessionResume` handlers temporarily so older tests
can be migrated task-by-task.

Implement `tunnel_close`:

```rust
async fn tunnel_close(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: remote_exec_proto::port_tunnel::TunnelCloseMeta = decode_frame_meta(&frame)?;
    close_attached_session(&tunnel, SessionCloseMode::GracefulClose).await;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelClosed,
            flags: 0,
            stream_id: 0,
            meta: encode_frame_meta(&meta)?,
            data: Vec::new(),
        })
        .await
}
```

- [ ] **Step 5: Run focused verification**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-daemon/src/port_forward.rs crates/remote-exec-host/src/port_forward/tunnel.rs crates/remote-exec-host/src/port_forward/session.rs crates/remote-exec-host/src/port_forward/mod.rs crates/remote-exec-daemon/tests/port_forward_rpc.rs crates/remote-exec-daemon/tests/support
git commit -m "feat: add rust daemon port tunnel v4 control"
```

---

### Task 4: Broker PortTunnel Lifecycle and v4 Upgrade Client

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports port_tunnel_lifecycle -- --exact`

**Testing approach:** TDD
Reason: explicit tunnel shutdown can be tested with controlled in-memory I/O before full forwarding logic is changed.

- [ ] **Step 1: Add a failing broker lifecycle test**

In `crates/remote-exec-broker/src/port_forward/tunnel.rs`, add:

```rust
#[tokio::test]
async fn port_tunnel_close_stops_reader_and_writer_tasks() {
    let (broker_side, mut daemon_side) = tokio::io::duplex(4096);
    let tunnel = PortTunnel::from_stream(broker_side).unwrap();
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: serde_json::to_vec(&remote_exec_proto::port_tunnel::TunnelCloseMeta {
                forward_id: "fwd_test".to_string(),
                generation: 1,
                reason: "operator_close".to_string(),
            })
            .unwrap(),
            data: Vec::new(),
        })
        .await
        .unwrap();

    let close = remote_exec_proto::port_tunnel::read_frame(&mut daemon_side)
        .await
        .unwrap();
    assert_eq!(close.frame_type, FrameType::TunnelClose);
    drop(daemon_side);
    tunnel.abort().await;
    tunnel.wait_closed(std::time::Duration::from_secs(1)).await.unwrap();
}
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --lib port_tunnel_close_stops_reader_and_writer_tasks -- --exact`
Expected: FAIL because `abort()` and `wait_closed()` do not exist.

- [ ] **Step 3: Add owned task lifecycle to `PortTunnel`**

Change `PortTunnel` to store join handles and cancellation:

```rust
pub struct PortTunnel {
    tx: mpsc::Sender<QueuedFrame>,
    rx: Mutex<mpsc::Receiver<anyhow::Result<Frame>>>,
    cancel: CancellationToken,
    reader_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    writer_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    queued_bytes: Arc<tokio::sync::Semaphore>,
}
```

Use `QueuedFrame { frame, charge }` so the writer releases byte budget after
write. Add:

```rust
pub async fn abort(&self) {
    self.cancel.cancel();
}

pub async fn wait_closed(&self, timeout: std::time::Duration) -> anyhow::Result<()> {
    if let Some(task) = self.reader_task.lock().await.take() {
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel reader task"))?
            .map_err(|err| anyhow::anyhow!("port tunnel reader task join failed: {err}"))?;
    }
    if let Some(task) = self.writer_task.lock().await.take() {
        tokio::time::timeout(timeout, task)
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel writer task"))?
            .map_err(|err| anyhow::anyhow!("port tunnel writer task join failed: {err}"))?;
    }
    Ok(())
}
```

In `DaemonClient::port_tunnel`, add a timeout around `send()` plus
`response.upgrade()`:

```rust
let response = tokio::time::timeout(PORT_TUNNEL_UPGRADE_TIMEOUT, request.send())
    .await
    .map_err(|_| DaemonClientError::Transport(anyhow::anyhow!("port tunnel upgrade timed out")))?
    .map_err(|err| self.rpc_transport_error("/v1/port/tunnel", started, err))?;
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-broker --lib port_tunnel_close_stops_reader_and_writer_tasks -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_opens_lists_and_closes_local_tcp_forward -- --exact`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/daemon_client.rs crates/remote-exec-broker/src/port_forward/tunnel.rs crates/remote-exec-broker/src/port_forward/mod.rs
git commit -m "feat: own port tunnel lifecycle"
```

---

### Task 5: Broker v4 Open and Close Flow

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: `crates/remote-exec-broker/src/tools/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_opens_lists_and_closes_local_tcp_forward -- --exact`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_close_reports_listen_cleanup_failures -- --exact`

**Testing approach:** TDD
Reason: public open/list/close behavior is the safest seam for switching broker runtime protocol.

- [ ] **Step 1: Add failing public state assertions to open/list/close tests**

In `forward_ports_opens_lists_and_closes_local_tcp_forward`, after open:

```rust
let entry = &open.structured_content["forwards"][0];
assert_eq!(entry["status"], "open");
assert_eq!(entry["phase"], "ready");
assert_eq!(entry["listen_state"]["health"], "ready");
assert_eq!(entry["connect_state"]["health"], "ready");
assert_eq!(entry["listen_state"]["generation"], 1);
assert_eq!(entry["connect_state"]["generation"], 1);
```

After close:

```rust
assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
assert_eq!(close.structured_content["forwards"][0]["phase"], "closed");
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_opens_lists_and_closes_local_tcp_forward -- --exact`
Expected: FAIL until broker entries include v4 public fields and open/close uses v4-ready construction.

- [ ] **Step 3: Implement broker v4 tunnel open helpers**

In `supervisor.rs`, replace `open_listen_session` with v4:

```rust
async fn open_listen_tunnel(
    side: &SideHandle,
    forward_id: &str,
    protocol: PublicForwardPortProtocol,
    generation: u64,
    resume_session_id: Option<String>,
) -> anyhow::Result<OpenListenSession> {
    let tunnel = open_connect_tunnel(side).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelOpen,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&remote_exec_proto::port_tunnel::TunnelOpenMeta {
                forward_id: forward_id.to_string(),
                role: remote_exec_proto::port_tunnel::TunnelRole::Listen,
                side: side.name().to_string(),
                generation,
                protocol: tunnel_protocol(protocol),
                resume_session_id,
            })?,
            data: Vec::new(),
        })
        .await?;
    wait_for_tunnel_ready(&tunnel, side, generation).await
}
```

Add equivalent `open_data_tunnel` for connect role. The helper
`tunnel_protocol` maps public protocol to tunnel protocol:

```rust
fn tunnel_protocol(
    protocol: PublicForwardPortProtocol,
) -> remote_exec_proto::port_tunnel::TunnelForwardProtocol {
    match protocol {
        PublicForwardPortProtocol::Tcp => remote_exec_proto::port_tunnel::TunnelForwardProtocol::Tcp,
        PublicForwardPortProtocol::Udp => remote_exec_proto::port_tunnel::TunnelForwardProtocol::Udp,
    }
}
```

- [ ] **Step 4: Use `ForwardPortEntry::new_open` during open**

In `open_tcp_forward` and `open_udp_forward`, generate `forward_id` before
opening tunnels and construct entries through `ForwardPortEntry::new_open(...)`
with a default broker limit summary:

```rust
let limits = default_forward_limit_summary();
let entry = ForwardPortEntry::new_open(
    forward_id,
    listen_side.name().to_string(),
    listen_response,
    connect_side.name().to_string(),
    connect_endpoint,
    spec.protocol,
    limits,
);
```

- [ ] **Step 5: Implement v4 close helper**

Update `close_listen_session` to send `TunnelClose` for the current generation,
then fall back to abort if acknowledgement times out:

```rust
async fn close_tunnel_generation(
    tunnel: &Arc<PortTunnel>,
    forward_id: &str,
    generation: u64,
    reason: &str,
) -> anyhow::Result<()> {
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelClose,
            flags: 0,
            stream_id: 0,
            meta: encode_tunnel_meta(&remote_exec_proto::port_tunnel::TunnelCloseMeta {
                forward_id: forward_id.to_string(),
                generation,
                reason: reason.to_string(),
            })?,
            data: Vec::new(),
        })
        .await?;
    wait_for_tunnel_closed(tunnel, generation).await
}
```

Preserve stream-level listener close acknowledgement for retained listener
cleanup until daemon-side `TunnelClose` fully owns retained cleanup.

- [ ] **Step 6: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_opens_lists_and_closes_local_tcp_forward -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_close_reports_listen_cleanup_failures -- --exact`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-broker/src/port_forward/tunnel.rs crates/remote-exec-broker/src/port_forward/store.rs crates/remote-exec-broker/src/tools/port_forward.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs crates/remote-exec-broker/tests/support/stub_daemon.rs
git commit -m "feat: open and close forwards with v4 tunnels"
```

---

### Task 6: Public Phase, Counters, and Store Updates

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/tools/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_closes_active_tcp_streams_after_connect_tunnel_drop -- --exact`

**Testing approach:** TDD
Reason: counters and phases are public observable state, and existing reconnect tests already create real stream loss.

- [ ] **Step 1: Add failing counter assertions**

In `forward_ports_closes_active_tcp_streams_after_connect_tunnel_drop`, after
the forward returns to open, add:

```rust
let forward = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
assert_eq!(forward["phase"], "ready");
assert_eq!(forward["dropped_tcp_streams"], 1);
assert!(forward["reconnect_attempts"].as_u64().unwrap() >= 1);
assert_eq!(forward["connect_state"]["health"], "ready");
```

Add a helper assertion to any UDP reconnect test:

```rust
assert!(
    forward["dropped_udp_datagrams"].as_u64().unwrap_or_default() >= 1,
    "expected at least one dropped UDP datagram during reconnect"
);
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_closes_active_tcp_streams_after_connect_tunnel_drop -- --exact`
Expected: FAIL until counters are updated.

- [ ] **Step 3: Add store update methods**

In `store.rs`, add methods:

```rust
pub async fn update_entry(
    &self,
    forward_id: &str,
    update: impl FnOnce(&mut ForwardPortEntry),
) {
    let mut entries = self.entries.write().await;
    if let Some(record) = entries.get_mut(forward_id) {
        update(&mut record.entry);
    }
}

pub async fn mark_reconnecting(
    &self,
    forward_id: &str,
    role: ForwardPortSideRole,
    error: String,
) {
    self.update_entry(forward_id, |entry| {
        entry.phase = ForwardPortPhase::Reconnecting;
        entry.reconnect_attempts += 1;
        entry.last_reconnect_at = Some(unix_timestamp_string());
        let side = match role {
            ForwardPortSideRole::Listen => &mut entry.listen_state,
            ForwardPortSideRole::Connect => &mut entry.connect_state,
        };
        side.health = ForwardPortSideHealth::Reconnecting;
        side.last_error = Some(error);
    })
    .await;
}
```

Add a tiny broker-local timestamp helper that uses the standard library, so no
new dependency is needed:

```rust
fn unix_timestamp_string() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
```

Use it in `mark_reconnecting`:

```rust
entry.last_reconnect_at = Some(unix_timestamp_string());
```

- [ ] **Step 4: Increment counters from bridges**

In `tcp_bridge.rs`, call store/runtime counter hooks when closing active streams
after tunnel loss:

```rust
runtime.counters.record_dropped_tcp_streams(dropped_count).await;
```

Add this field to `ForwardRuntime` if it is not already present:

```rust
pub(super) store: super::store::PortForwardStore,
```

and update call sites:

```rust
runtime.store.update_entry(&runtime.forward_id, |entry| {
    entry.dropped_tcp_streams += dropped_count as u64;
    entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(dropped_count as u64);
}).await;
```

In `udp_bridge.rs`, increment datagram drops whenever a datagram cannot be
forwarded because the connect tunnel is recovering or the connector map was
reset:

```rust
runtime.store.update_entry(&runtime.forward_id, |entry| {
    entry.dropped_udp_datagrams += 1;
}).await;
```

- [ ] **Step 5: Format text output with phase**

In `tools/port_forward.rs`, update `format_forward_ports_text` so entries with
non-ready phase include it:

```rust
let phase_suffix = match entry.phase {
    ForwardPortPhase::Ready | ForwardPortPhase::Closed | ForwardPortPhase::Failed => String::new(),
    phase => format!(", phase={}", format_phase(phase)),
};
```

Add `format_phase` mapping all phase variants to snake-case strings.

- [ ] **Step 6: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_closes_active_tcp_streams_after_connect_tunnel_drop -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_forward_open_after_listen_tunnel_drop -- --exact`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/store.rs crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/src/port_forward/udp_bridge.rs crates/remote-exec-broker/src/tools/port_forward.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "feat: expose port forward runtime phase"
```

---

### Task 7: Symmetric Reconnect Policy and Operation Timeouts

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/events.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_times_out_stalled_resume_attempts -- --exact`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_connect_side_reconnect_retries_transient_open_failures -- --exact`

**Testing approach:** TDD
Reason: retry budgets and timeout behavior must be driven by failure injection to prevent accidental infinite waits.

- [ ] **Step 1: Add failing connect-side retry test**

In `mcp_forward_ports.rs`, add:

```rust
#[tokio::test]
async fn forward_ports_connect_side_reconnect_retries_transient_open_failures() {
    let fixture = support::spawners::spawn_broker_with_local_and_stub_port_forward_version(4).await;
    support::stub_daemon::enable_reconnectable_port_tunnel(&fixture.stub_state).await;
    support::stub_daemon::fail_next_port_tunnel_upgrades(&fixture.stub_state, 2).await;

    let echo = spawn_tcp_echo().await;
    let open = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "builder-a",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": echo.to_string(),
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let forward_id = forward_id_from(&open);
    let listen_endpoint = listen_endpoint_from(&open);

    support::stub_daemon::force_close_port_tunnel_transport(&fixture.stub_state).await;
    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    let _ = stream.write_all(b"trigger").await;
    drop(stream);

    let forward = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
    assert_eq!(forward["phase"], "ready");
    assert!(forward["reconnect_attempts"].as_u64().unwrap() >= 1);
}
```

Add `fail_next_port_tunnel_upgrades` to the stub daemon control by decrementing
a counter before upgrade and returning a transport drop while the counter is
positive.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_connect_side_reconnect_retries_transient_open_failures -- --exact`
Expected: FAIL until connect-side open retries use the shared policy.

- [ ] **Step 3: Add reconnect policy type and retry helper**

In `supervisor.rs`, add:

```rust
#[derive(Clone, Copy)]
pub(super) struct PortForwardReconnectPolicy {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub attempt_timeout: Duration,
    pub total_timeout: Duration,
    pub max_attempts: Option<u32>,
}

impl PortForwardReconnectPolicy {
    pub(super) fn listen(resume_timeout: Duration) -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: Duration::from_secs(2),
            total_timeout: effective_resume_timeout(resume_timeout),
            max_attempts: None,
        }
    }

    pub(super) fn connect() -> Self {
        Self {
            initial_backoff: LISTEN_RECONNECT_INITIAL_BACKOFF,
            max_backoff: LISTEN_RECONNECT_MAX_BACKOFF,
            attempt_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(10),
            max_attempts: None,
        }
    }
}
```

Add:

```rust
async fn retry_reconnect<T, Fut>(
    cancel: CancellationToken,
    policy: PortForwardReconnectPolicy,
    mut attempt_fn: impl FnMut() -> Fut,
) -> anyhow::Result<Option<T>>
where
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let deadline = Instant::now() + policy.total_timeout;
    let mut backoff = policy.initial_backoff;
    let mut attempts = 0u32;
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if policy.max_attempts.is_some_and(|max| attempts >= max) {
            return Err(anyhow::anyhow!("port forward reconnect attempts exhausted"));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(anyhow::anyhow!("port tunnel reconnect timed out"));
        }
        attempts += 1;
        let attempt_timeout = policy.attempt_timeout.min(remaining);
        let result = tokio::select! {
            _ = cancel.cancelled() => return Ok(None),
            result = tokio::time::timeout(attempt_timeout, attempt_fn()) => result,
        };
        match result {
            Ok(Ok(value)) => return Ok(Some(value)),
            Ok(Err(err)) if is_retryable_transport_error(&err) => {}
            Ok(Err(err)) => return Err(err),
            Err(_) => {}
        }
        let sleep_for = backoff.min(deadline.saturating_duration_since(Instant::now()));
        tokio::select! {
            _ = cancel.cancelled() => return Ok(None),
            _ = tokio::time::sleep(sleep_for) => {}
        }
        backoff = std::cmp::min(backoff + backoff, policy.max_backoff);
    }
}
```

- [ ] **Step 4: Use shared policy for both tunnel roles**

Refactor `reconnect_listen_tunnel` to call `retry_reconnect` with
`PortForwardReconnectPolicy::listen(...)`.

Add:

```rust
pub(super) async fn reconnect_connect_tunnel(
    runtime: &ForwardRuntime,
) -> anyhow::Result<Option<Arc<PortTunnel>>> {
    runtime
        .store
        .mark_reconnecting(
            &runtime.forward_id,
            ForwardPortSideRole::Connect,
            "connect-side transport loss".to_string(),
        )
        .await;
    retry_reconnect(
        runtime.cancel.clone(),
        PortForwardReconnectPolicy::connect(),
        || async { open_connect_tunnel(&runtime.connect_side).await },
    )
    .await
}
```

Update TCP/UDP bridge `RecoverTunnel(TunnelRole::Connect)` branches to call
`reconnect_connect_tunnel`.

- [ ] **Step 5: Add timeouts to open/listener ack paths**

Wrap waiting calls:

```rust
tokio::time::timeout(PORT_FORWARD_OPEN_ACK_TIMEOUT, wait_for_listener_ready(...))
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for port forward listener acknowledgement"))??;
```

Add constants in `mod.rs`:

```rust
const PORT_FORWARD_OPEN_ACK_TIMEOUT: Duration = Duration::from_secs(5);
const PORT_FORWARD_TUNNEL_READY_TIMEOUT: Duration = Duration::from_secs(5);
```

- [ ] **Step 6: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_times_out_stalled_resume_attempts -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_connect_side_reconnect_retries_transient_open_failures -- --exact`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/events.rs crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/src/port_forward/udp_bridge.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs crates/remote-exec-broker/tests/support/stub_daemon.rs
git commit -m "feat: add symmetric port forward reconnect policy"
```

---

### Task 8: Broker and Rust Daemon Resource Limits

**Files:**
- Create: `crates/remote-exec-broker/src/port_forward/limits.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `crates/remote-exec-broker/src/startup.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Modify: `crates/remote-exec-host/src/config/mod.rs`
- Modify: `crates/remote-exec-host/src/state.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session_store.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_open_when_forward_limit_is_reached -- --exact`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_rejects_retained_session_limit -- --exact`

**Testing approach:** TDD
Reason: limits are public safety behavior and should fail closed before creating unmanaged resources.

- [ ] **Step 1: Add failing broker limit test**

In `mcp_forward_ports.rs`, add:

```rust
#[tokio::test]
async fn forward_ports_rejects_open_when_forward_limit_is_reached() {
    let fixture = support::spawners::spawn_broker_local_only_with_port_forward_limit(1).await;
    let first = fixture
        .call_tool(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    let first_id = forward_id_from(&first);

    let error = fixture
        .call_tool_error(
            "forward_ports",
            serde_json::json!({
                "action": "open",
                "listen_side": "local",
                "connect_side": "local",
                "forwards": [{
                    "listen_endpoint": "127.0.0.1:0",
                    "connect_endpoint": "127.0.0.1:9",
                    "protocol": "tcp"
                }]
            }),
        )
        .await;
    assert!(error.contains("port_forward_limit_exceeded"), "unexpected error: {error}");

    let _ = fixture.call_tool(
        "forward_ports",
        serde_json::json!({"action": "close", "forward_ids": [first_id]}),
    ).await;
}
```

- [ ] **Step 2: Add failing Rust daemon retained-session limit test**

In `port_forward_rpc.rs`, add a fixture config helper with retained session
limit `1`, open one listen tunnel, then open a second and assert error code:

```rust
assert_eq!(error_meta.code, "port_tunnel_limit_exceeded");
```

- [ ] **Step 3: Verify red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_open_when_forward_limit_is_reached -- --exact`
Expected: FAIL until broker limits exist.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_rejects_retained_session_limit -- --exact`
Expected: FAIL until daemon limits exist.

- [ ] **Step 4: Implement broker limit helpers**

Create `crates/remote-exec-broker/src/port_forward/limits.rs`:

```rust
use remote_exec_proto::public::ForwardPortLimitSummary;

#[derive(Debug, Clone, Copy)]
pub struct BrokerPortForwardLimits {
    pub max_open_forwards_total: usize,
    pub max_forwards_per_side_pair: usize,
    pub max_active_tcp_streams_per_forward: u64,
    pub max_pending_tcp_bytes_per_stream: u64,
    pub max_pending_tcp_bytes_per_forward: u64,
    pub max_udp_peers_per_forward: u64,
    pub max_tunnel_queued_bytes: u64,
    pub max_reconnecting_forwards: usize,
}

impl Default for BrokerPortForwardLimits {
    fn default() -> Self {
        Self {
            max_open_forwards_total: 64,
            max_forwards_per_side_pair: 16,
            max_active_tcp_streams_per_forward: 256,
            max_pending_tcp_bytes_per_stream: 256 * 1024,
            max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
            max_udp_peers_per_forward: 256,
            max_tunnel_queued_bytes: 8 * 1024 * 1024,
            max_reconnecting_forwards: 16,
        }
    }
}

impl BrokerPortForwardLimits {
    pub fn public_summary(self) -> ForwardPortLimitSummary {
        ForwardPortLimitSummary {
            max_active_tcp_streams: self.max_active_tcp_streams_per_forward,
            max_udp_peers: self.max_udp_peers_per_forward,
            max_pending_tcp_bytes_per_stream: self.max_pending_tcp_bytes_per_stream,
            max_pending_tcp_bytes_per_forward: self.max_pending_tcp_bytes_per_forward,
            max_tunnel_queued_bytes: self.max_tunnel_queued_bytes,
        }
    }
}
```

Export it from `mod.rs`, add `#[serde(default)] pub port_forward_limits:
BrokerPortForwardLimits` to `BrokerConfig`, and add `pub port_forward_limits:
BrokerPortForwardLimits` to `BrokerState`. In `build_state` in
`crates/remote-exec-broker/src/startup.rs`, copy `config.port_forward_limits`
into `BrokerState`.

Derive `Deserialize` on `BrokerPortForwardLimits` and add a validation method:

```rust
impl BrokerPortForwardLimits {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(self.max_open_forwards_total > 0, "port_forward_limits.max_open_forwards_total must be greater than zero");
        anyhow::ensure!(self.max_forwards_per_side_pair > 0, "port_forward_limits.max_forwards_per_side_pair must be greater than zero");
        anyhow::ensure!(self.max_active_tcp_streams_per_forward > 0, "port_forward_limits.max_active_tcp_streams_per_forward must be greater than zero");
        anyhow::ensure!(self.max_pending_tcp_bytes_per_stream > 0, "port_forward_limits.max_pending_tcp_bytes_per_stream must be greater than zero");
        anyhow::ensure!(self.max_pending_tcp_bytes_per_forward >= self.max_pending_tcp_bytes_per_stream, "port_forward_limits.max_pending_tcp_bytes_per_forward must be at least max_pending_tcp_bytes_per_stream");
        anyhow::ensure!(self.max_udp_peers_per_forward > 0, "port_forward_limits.max_udp_peers_per_forward must be greater than zero");
        anyhow::ensure!(self.max_tunnel_queued_bytes > 0, "port_forward_limits.max_tunnel_queued_bytes must be greater than zero");
        anyhow::ensure!(self.max_reconnecting_forwards > 0, "port_forward_limits.max_reconnecting_forwards must be greater than zero");
        Ok(())
    }
}
```

Call `self.port_forward_limits.validate()?` from `BrokerConfig::validate()`.

- [ ] **Step 5: Enforce broker limits before open**

Add store helpers:

```rust
pub async fn open_count(&self) -> usize {
    self.entries.read().await.len()
}

pub async fn side_pair_count(&self, listen_side: &str, connect_side: &str) -> usize {
    self.entries
        .read()
        .await
        .values()
        .filter(|record| {
            record.entry.listen_side == listen_side && record.entry.connect_side == connect_side
        })
        .count()
}
```

Before opening:

```rust
anyhow::ensure!(
    state.port_forwards.open_count().await + forwards.len() <= limits.max_open_forwards_total,
    "port_forward_limit_exceeded: broker open forward limit reached"
);
```

- [ ] **Step 6: Enforce Rust daemon session limits**

In `TunnelSessionStore`, add limit-aware insert:

```rust
pub(super) async fn try_insert(
    &self,
    session: Arc<SessionState>,
    max_sessions: usize,
) -> Result<(), HostRpcError> {
    let mut sessions = self.sessions.lock().await;
    if sessions.len() >= max_sessions {
        return Err(rpc_error(
            "port_tunnel_limit_exceeded",
            "retained port tunnel session limit reached",
        ));
    }
    sessions.insert(session.id.clone(), session);
    Ok(())
}
```

Add `HostPortForwardLimits` to `crates/remote-exec-host/src/config/mod.rs`:

```rust
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct HostPortForwardLimits {
    pub max_tunnel_connections: usize,
    pub max_retained_sessions: usize,
    pub max_retained_listeners: usize,
    pub max_udp_binds: usize,
    pub max_active_tcp_streams: usize,
    pub max_tunnel_queued_bytes: usize,
}

impl Default for HostPortForwardLimits {
    fn default() -> Self {
        Self {
            max_tunnel_connections: 128,
            max_retained_sessions: 64,
            max_retained_listeners: 64,
            max_udp_binds: 64,
            max_active_tcp_streams: 1024,
            max_tunnel_queued_bytes: 8 * 1024 * 1024,
        }
    }
}
```

Add `pub port_forward_limits: HostPortForwardLimits` to `HostRuntimeConfig` and
`EmbeddedHostConfig`, and copy the field through `EmbeddedHostConfig::
into_host_runtime_config()`. Add `#[serde(default)] pub port_forward_limits:
HostPortForwardLimits` to `DaemonConfig`, return it from `host_runtime_config()`
and `into_host_runtime_config()`, and copy it through `EmbeddedDaemonConfig` and
the `From<EmbeddedHostConfig>` conversions.

Add `#[serde(default)] pub port_forward_limits: HostPortForwardLimits` to
`LocalTargetConfig` in `crates/remote-exec-broker/src/config.rs`, and copy that
field into the `EmbeddedHostConfig` built by `LocalTargetConfig::
embedded_host_config()`. Use
`state.config.port_forward_limits.max_retained_sessions` in the daemon
`try_insert` call.

- [ ] **Step 7: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_rejects_open_when_forward_limit_is_reached -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_rejects_retained_session_limit -- --exact`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/limits.rs crates/remote-exec-broker/src/port_forward/mod.rs crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-broker/src/port_forward/store.rs crates/remote-exec-host/src/port_forward/session_store.rs crates/remote-exec-host/src/port_forward/tunnel.rs crates/remote-exec-daemon/tests/port_forward_rpc.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "feat: add port forward resource limits"
```

---

### Task 9: Byte-Aware Backpressure and Drop Counters

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_closes_pending_tcp_stream_when_remote_connect_never_acknowledges -- --exact`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_drops_udp_datagrams_under_pressure -- --exact`

**Testing approach:** TDD
Reason: pressure behavior must be deterministic and measured by public counters.

- [ ] **Step 1: Add failing UDP pressure test**

In `mcp_forward_ports.rs`, add a test that opens a UDP forward to a stub daemon
that hangs connect tunnel writes, sends datagrams until pressure is exceeded,
then asserts:

```rust
let listed = fixture
    .call_tool(
        "forward_ports",
        serde_json::json!({"action": "list", "forward_ids": [forward_id]}),
    )
    .await;
let drops = listed.structured_content["forwards"][0]["dropped_udp_datagrams"]
    .as_u64()
    .unwrap();
assert!(drops > 0, "expected UDP drops under pressure");
assert_eq!(listed.structured_content["forwards"][0]["status"], "open");
```

- [ ] **Step 2: Verify red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_drops_udp_datagrams_under_pressure -- --exact`
Expected: FAIL until UDP pressure increments counters.

- [ ] **Step 3: Charge queued bytes in `PortTunnel::send`**

Add:

```rust
fn frame_charge(frame: &Frame) -> usize {
    remote_exec_proto::port_tunnel::HEADER_LEN
        .saturating_add(frame.meta.len())
        .saturating_add(frame.data.len())
}
```

Before queueing data frames, check a byte budget:

```rust
let charge = frame_charge(&frame);
if charge > self.max_queued_bytes {
    return Err(anyhow::anyhow!("port_forward_backpressure_exceeded: frame exceeds tunnel queue budget"));
}
```

Use a semaphore or atomic counter to track queued bytes. Control frames
(`stream_id == 0`) bypass the data budget but still fail if the writer channel
is closed.

- [ ] **Step 4: Close/drop affected data under pressure**

In TCP paths, when `send` returns `port_forward_backpressure_exceeded`, close
the paired stream and increment `dropped_tcp_streams`.

In UDP paths, when `send` returns `port_forward_backpressure_exceeded`, drop the
datagram and increment `dropped_udp_datagrams` without failing the forward.

- [ ] **Step 5: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_closes_pending_tcp_stream_when_remote_connect_never_acknowledges -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_drops_udp_datagrams_under_pressure -- --exact`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/tunnel.rs crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/src/port_forward/udp_bridge.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "feat: add byte-aware port tunnel pressure handling"
```

---

### Task 10: Generation-Safe Stream ID Allocation

**Files:**
- Create: `crates/remote-exec-broker/src/port_forward/generation.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Test/Verify: `cargo test -p remote-exec-broker --lib stream_id_allocator_rotates_before_wrap -- --exact`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_rejects_old_generation_frames -- --exact`

**Testing approach:** TDD
Reason: allocator wraparound is a pure logic seam; daemon generation rejection is a protocol seam.

- [ ] **Step 1: Add failing allocator unit test**

Create `generation.rs` with tests:

```rust
#[test]
fn stream_id_allocator_rotates_before_wrap() {
    let mut allocator = StreamIdAllocator::new_odd();
    allocator.set_next_for_test(u32::MAX - 2);
    assert_eq!(allocator.next().unwrap(), u32::MAX - 2);
    assert!(allocator.next().is_none());
    assert!(allocator.needs_generation_rotation());
}
```

- [ ] **Step 2: Verify red state**

Run: `cargo test -p remote-exec-broker --lib stream_id_allocator_rotates_before_wrap -- --exact`
Expected: FAIL until allocator exists.

- [ ] **Step 3: Implement broker allocator**

In `generation.rs`:

```rust
#[derive(Debug, Clone)]
pub(super) struct StreamIdAllocator {
    next: u32,
    step: u32,
    exhausted: bool,
}

impl StreamIdAllocator {
    pub(super) fn new_odd() -> Self {
        Self { next: 1, step: 2, exhausted: false }
    }

    pub(super) fn new_odd_from(start: u32) -> Self {
        Self { next: start, step: 2, exhausted: false }
    }

    pub(super) fn next(&mut self) -> Option<u32> {
        if self.exhausted {
            return None;
        }
        let value = self.next;
        match self.next.checked_add(self.step) {
            Some(next) => self.next = next,
            None => self.exhausted = true,
        }
        Some(value)
    }

    pub(super) fn needs_generation_rotation(&self) -> bool {
        self.exhausted
    }

    #[cfg(test)]
    pub(super) fn set_next_for_test(&mut self, next: u32) {
        self.next = next;
        self.exhausted = false;
    }
}
```

Use `StreamIdAllocator` in TCP and UDP bridge files instead of
`checked_add(2).unwrap_or(1)`.

- [ ] **Step 4: Add daemon generation checks**

Store the attached generation in Rust `SessionState` and `TunnelState`. Reject
frames with mismatched generation metadata for control frames:

```rust
return Err(rpc_error(
    "port_tunnel_generation_mismatch",
    format!("frame generation `{}` does not match tunnel generation `{}`", frame_generation, current_generation),
));
```

For data frames, the tunnel generation is implicit in the attached tunnel; old
generation frames cannot arrive on a new stream unless a stale transport is
still active. If a stale attachment sends after replacement, its attachment
cancel token must already be cancelled by `attach_session_to_tunnel`.

- [ ] **Step 5: Run focused verification**

Run: `cargo test -p remote-exec-broker --lib stream_id_allocator_rotates_before_wrap -- --exact`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc port_tunnel_rejects_old_generation_frames -- --exact`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward/generation.rs crates/remote-exec-broker/src/port_forward/mod.rs crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/src/port_forward/udp_bridge.rs crates/remote-exec-host/src/port_forward/session.rs crates/remote-exec-host/src/port_forward/tcp.rs crates/remote-exec-host/src/port_forward/udp.rs crates/remote-exec-daemon/tests/port_forward_rpc.rs
git commit -m "feat: add generation-safe port stream ids"
```

---

### Task 11: C++ Daemon v4 Parity

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`

**Testing approach:** characterization/integration test
Reason: C++ must match the Rust v4 wire contract and existing C++ daemon tests are the most reliable executable specification.

- [ ] **Step 1: Add failing C++ frame tests**

In `test_port_tunnel_frame.cpp`, add assertions that numeric values round-trip:

```cpp
PortTunnelFrame frame;
frame.type = PortTunnelFrameType::TunnelOpen;
frame.flags = 0U;
frame.stream_id = 0U;
frame.meta = "{\"forward_id\":\"fwd_test\",\"generation\":1}";
std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
PortTunnelFrame decoded = decode_port_tunnel_frame(bytes);
assert(decoded.type == PortTunnelFrameType::TunnelOpen);
assert(decoded.stream_id == 0U);
assert(decoded.meta == frame.meta);
```

Add similar tests for `TunnelReady`, `TunnelClose`, and `TunnelClosed`.

- [ ] **Step 2: Verify red state**

Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`
Expected: FAIL until enum values exist.

- [ ] **Step 3: Extend C++ frame enum and upgrade header**

In `port_tunnel_frame.h`, add the same enum values and numeric IDs as Rust:

```cpp
TunnelOpen = 7,
TunnelReady = 8,
TunnelClose = 9,
TunnelClosed = 17,
TunnelHeartbeat = 18,
TunnelHeartbeatAck = 19,
ForwardRecovering = 20,
ForwardRecovered = 21,
```

In `port_tunnel_frame.cpp`, update `frame_type_from_byte`.

In `port_tunnel_transport.cpp`, change upgrade validation to:

```cpp
request.header("x-remote-exec-port-tunnel-version") != "4"
```

- [ ] **Step 4: Implement C++ v4 session open/close**

In `PortTunnelConnection::handle_frame`, route new control frame types. Keep old
`SessionOpen` and `SessionResume` temporarily if existing tests still cover
them.

Implement `tunnel_open` by parsing JSON metadata:

```cpp
const Json meta = Json::parse(frame.meta);
const std::string role = meta.at("role").get<std::string>();
const uint64_t generation = meta.at("generation").get<uint64_t>();
if (role == "listen") {
    // create or resume retained session, attach, send TunnelReady with session_id
} else if (role == "connect") {
    // mark connection generation, send TunnelReady without session_id
} else {
    throw PortForwardError(400, "invalid_port_tunnel", "unknown tunnel role");
}
```

Implement `tunnel_close` by calling `service_->close_session(session)` for
attached listen sessions, closing transport-owned state, and sending
`TunnelClosed`.

- [ ] **Step 5: Add C++ worker limit guard**

In `PortTunnelService`, add a simple atomic active worker counter:

```cpp
std::atomic<unsigned long> active_workers_;
unsigned long max_workers_;
```

Before spawning listener/read/expiry threads, increment only when below
`max_workers_`; otherwise send:

```json
{"code":"port_tunnel_limit_exceeded","message":"port tunnel worker limit reached","fatal":true}
```

Decrement on thread exit.

- [ ] **Step 6: Run focused C++ verification**

Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 7: Run broker C++ parity verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "feat: add cpp port tunnel v4 parity"
```

---

### Task 12: End-To-End Coverage and Documentation

**Files:**
- Modify: `tests/e2e/multi_target.rs`
- Modify: `tests/e2e/support/mod.rs`
- Modify: `README.md`
- Modify: `configs/broker.example.toml`
- Modify: `configs/daemon.example.toml`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `cargo test --test multi_target`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing tests + targeted verification
Reason: by this point focused tests cover behavior; this task locks full system behavior and public docs.

- [ ] **Step 1: Add e2e public state assertions**

In `tests/e2e/multi_target.rs`, update reconnect tests to assert:

```rust
let forward = support::wait_for_forward_status_timeout(
    &cluster.broker,
    &forward_id,
    "open",
    Duration::from_secs(5),
)
.await
.expect("forward should stay open after reconnect");
assert_eq!(forward["phase"], "ready");
assert!(forward["reconnect_attempts"].as_u64().unwrap() >= 1);
```

In connect-side tunnel drop tests, also assert:

```rust
assert!(forward["dropped_tcp_streams"].as_u64().unwrap_or_default() >= 1);
```

For UDP reconnect tests, assert:

```rust
assert!(forward["dropped_udp_datagrams"].as_u64().unwrap_or_default() >= 1);
```

- [ ] **Step 2: Run e2e verification**

Run: `cargo test --test multi_target`
Expected: PASS.

- [ ] **Step 3: Update README reliability and trust model**

In `README.md`, update forwarding notes to include:

```markdown
- `forward_ports` v4 uses `x-remote-exec-port-tunnel-version: 4` for daemon-private HTTP/1.1 Upgrade tunnels.
- `forward_ports list` includes `phase`, per-side health, generation, reconnect counters, dropped TCP stream counters, dropped UDP datagram counters, and effective limits.
- A forward with legacy `status = "open"` may have `phase = "reconnecting"` while the broker is recovering a tunnel.
- Active TCP streams are closed across listen-side or connect-side tunnel reconnect. UDP per-peer connector state is recreated after reconnect, and datagrams that cannot be relayed under pressure are counted as drops.
- Broker and daemon forwarding limits protect open forwards, retained sessions, active TCP streams, UDP peers, queued tunnel bytes, and C++ forwarding worker threads.
```

In the trust model section, keep the network pivot warning and add:

```markdown
- `forward_ports` network access is controlled by target selection and static forwarding limits, not filesystem sandbox rules.
```

- [ ] **Step 4: Update config examples**

Add commented forwarding limit examples to `configs/broker.example.toml`:

```toml
# [port_forward_limits]
# max_open_forwards_total = 64
# max_forwards_per_side_pair = 16
# max_active_tcp_streams_per_forward = 256
# max_pending_tcp_bytes_per_stream = 262144
# max_pending_tcp_bytes_per_forward = 2097152
# max_udp_peers_per_forward = 256
# max_tunnel_queued_bytes = 8388608
# max_reconnecting_forwards = 16
```

Add daemon limit examples to `configs/daemon.example.toml`:

```toml
# [port_forward_limits]
# max_tunnel_connections = 128
# max_retained_sessions = 64
# max_retained_listeners = 64
# max_udp_binds = 64
# max_active_tcp_streams = 1024
# max_tunnel_queued_bytes = 8388608
```

Add C++ INI examples to `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`:

```ini
# port_forward_max_tunnel_connections = 128
# port_forward_max_retained_sessions = 64
# port_forward_max_retained_listeners = 64
# port_forward_max_udp_binds = 64
# port_forward_max_active_tcp_streams = 1024
# port_forward_max_tunnel_queued_bytes = 8388608
# port_forward_max_worker_threads = 256
```

- [ ] **Step 5: Update skill guidance**

In `skills/using-remote-exec-mcp/SKILL.md`, update the `forward_ports` section
to say:

```markdown
- Prefer checking `phase` and side health, not only legacy `status`, when waiting for forwarded services to be ready.
- `status = "open"` is compatible with older clients; `phase = "reconnecting"` means new work may fail until recovery returns to `phase = "ready"`.
- For v4 targets, `port_forward_protocol_version` and `x-remote-exec-port-tunnel-version` are both `4`.
```

- [ ] **Step 6: Run full quality gate**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add tests/e2e/multi_target.rs tests/e2e/support/mod.rs README.md configs/broker.example.toml configs/daemon.example.toml crates/remote-exec-daemon-cpp/README.md crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini skills/using-remote-exec-mcp/SKILL.md
git commit -m "docs: document port forward v4 hardening"
```

---

## Final Verification

Run these commands after all tasks:

```bash
cargo test -p remote-exec-proto
cargo test -p remote-exec-daemon --test port_forward_rpc
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
cargo test --test multi_target
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected final state:

- `list_targets` reports `forward_protocol=v4` for forwarding-capable targets.
- v4 tunnels require `x-remote-exec-port-tunnel-version: 4`.
- `forward_ports list` returns legacy `status` plus `phase`, side state, counters, and limits.
- explicit close releases broker tunnel tasks and daemon retained resources.
- listen-side and connect-side retryable transport loss use bounded reconnect.
- active TCP stream loss and UDP datagram drops are visible in counters.
- broker and daemon limits return stable error codes.
- stream IDs rotate by generation before wraparound.
- Rust daemon and C++ daemon pass parity coverage.
