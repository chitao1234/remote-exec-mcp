# Port Forward Upgrade Tunnel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Replace the internal `forward_ports` broker-daemon protocol with a full-duplex HTTP/1.1 Upgrade tunnel for Rust and C++ daemons, with no public MCP schema changes.

**Architecture:** Add a shared compact binary frame protocol (`REPFWD1\n` plus 16-byte headers), expose daemon-private `POST /v1/port/tunnel`, and make the broker relay TCP/UDP frames over long-lived upgraded connections instead of per-chunk JSON RPCs. Remove the old lease-renewed internal port-forward routes and make daemon resources tunnel-lifetime owned.

**Tech Stack:** Rust 2024, Tokio `AsyncRead`/`AsyncWrite`, reqwest HTTP upgrades, Hyper HTTP/1.1 upgrades, Axum, C++11 sockets/threads, C++17 C++ daemon tests.

---

## File Structure

- `crates/remote-exec-proto/src/port_tunnel.rs`: Rust frame constants, frame enum, metadata structs, async frame I/O helpers, and codec tests.
- `crates/remote-exec-proto/src/rpc.rs`: add internal `TargetInfoResponse::port_forward_protocol_version` and remove old lease/request structs once no Rust code uses them.
- `crates/remote-exec-host/src/port_forward.rs`: replace lease-owned request/response helpers with tunnel-owned TCP/UDP resource pumps and host tunnel tests.
- `crates/remote-exec-daemon/src/port_forward.rs`: add the Axum Upgrade handler for `/v1/port/tunnel`.
- `crates/remote-exec-daemon/src/http/routes.rs`: route only `/v1/port/tunnel` for port forwarding.
- `crates/remote-exec-daemon/src/tls.rs` and `crates/remote-exec-daemon/src/tls_enabled.rs`: enable Hyper `.with_upgrades()` for plain and TLS daemon HTTP serving.
- `crates/remote-exec-broker/src/daemon_client.rs`: add `port_tunnel()` upgrade client and remove old internal port-forward RPC methods.
- `crates/remote-exec-broker/src/port_forward.rs`: rewrite `forward_ports` runtime around tunnel pairs, public `forward_id` state, close/list behavior, and capability validation.
- `crates/remote-exec-broker/src/tools/targets.rs`, `crates/remote-exec-broker/src/target_config.rs`, and cached target-info paths: propagate the new internal protocol version without changing public schemas.
- `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h` and `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`: C++ frame codec.
- `crates/remote-exec-daemon-cpp/include/port_tunnel.h` and `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`: C++ tunnel engine and tunnel-scoped resource cleanup.
- `crates/remote-exec-daemon-cpp/src/server.cpp`, `src/server_routes.cpp`, and `src/server_route_port_forward.cpp`: C++ Upgrade route handling and old route removal.
- `crates/remote-exec-daemon-cpp/Makefile`: add tunnel sources/tests and update check targets.
- Tests: Rust proto/host/daemon/broker suites, C++ frame/server suites, Rust real-daemon e2e, C++ real-daemon broker e2e.
- Docs: `README.md`, `configs/broker.example.toml`, `crates/remote-exec-daemon-cpp/README.md`, and `skills/using-remote-exec-mcp/SKILL.md`.

---

### Task 1: Rust Frame Codec and Capability

**Files:**
- Create: `crates/remote-exec-proto/src/port_tunnel.rs`
- Modify: `crates/remote-exec-proto/src/lib.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-proto/Cargo.toml`
- Test/Verify: `cargo test -p remote-exec-proto port_tunnel --lib`

**Testing approach:** TDD
Reason: The frame format is isolated and has a direct encode/decode seam that should fail before the codec exists.

- [ ] **Step 1: Add failing codec and capability tests**

Create `crates/remote-exec-proto/src/port_tunnel.rs` with these tests. The test bodies should use `tokio::io::duplex`, `write_frame`, and `read_frame` directly; no production forwarding code is needed.

```rust
#[tokio::test]
async fn frame_round_trip_preserves_binary_payload() {
    let (mut client, mut server) = tokio::io::duplex(1024);
    let payload = vec![0, 1, 2, 255, b'R', b'\n'];
    let writer = tokio::spawn(async move {
        write_frame(
            &mut client,
            &Frame {
                frame_type: FrameType::TcpData,
                flags: 7,
                stream_id: 3,
                meta: br#"{"note":"binary"}"#.to_vec(),
                data: payload,
            },
        )
        .await
    });

    let frame = read_frame(&mut server).await.unwrap();
    writer.await.unwrap().unwrap();
    assert_eq!(frame.frame_type, FrameType::TcpData);
    assert_eq!(frame.flags, 7);
    assert_eq!(frame.stream_id, 3);
    assert_eq!(frame.meta, br#"{"note":"binary"}"#);
    assert_eq!(frame.data, vec![0, 1, 2, 255, b'R', b'\n']);
}

#[tokio::test]
async fn oversized_meta_is_rejected() {
    let mut bytes = Vec::from([FrameType::Error as u8, 0, 0, 0]);
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&((MAX_META_LEN as u32) + 1).to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    let err = read_frame(&mut bytes.as_slice()).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn oversized_data_is_rejected() {
    let mut bytes = Vec::from([FrameType::TcpData as u8, 0, 0, 0]);
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&((MAX_DATA_LEN as u32) + 1).to_be_bytes());
    let err = read_frame(&mut bytes.as_slice()).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
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
    })).unwrap();
    assert_eq!(info.port_forward_protocol_version, 0);
}
```

Expose the module with `pub mod port_tunnel;` in `crates/remote-exec-proto/src/lib.rs`, add `port_forward_protocol_version: u32` with `#[serde(default)]` to `TargetInfoResponse`, and add `tokio = { workspace = true, features = ["io-util", "macros"] }` to `crates/remote-exec-proto/Cargo.toml`.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-proto port_tunnel --lib`

Expected: FAIL because the codec functions/types referenced by tests are not implemented yet.

- [ ] **Step 3: Implement the codec**

Implement:

```rust
pub const PREFACE: &[u8; 8] = b"REPFWD1\n";
pub const HEADER_LEN: usize = 16;
pub const MAX_META_LEN: usize = 16 * 1024;
pub const MAX_DATA_LEN: usize = 256 * 1024;
pub const TUNNEL_PROTOCOL_VERSION_HEADER: &str = "x-remote-exec-port-tunnel-version";
pub const TUNNEL_PROTOCOL_VERSION: &str = "1";
pub const UPGRADE_TOKEN: &str = "remote-exec-port-tunnel";

#[repr(u8)]
pub enum FrameType { Error = 1, Close = 2, TcpListen = 10, TcpListenOk = 11, TcpAccept = 12, TcpConnect = 13, TcpConnectOk = 14, TcpData = 15, TcpEof = 16, UdpBind = 30, UdpBindOk = 31, UdpDatagram = 32 }

pub struct Frame { pub frame_type: FrameType, pub flags: u8, pub stream_id: u32, pub meta: Vec<u8>, pub data: Vec<u8> }

pub async fn write_preface<W: AsyncWrite + Unpin>(writer: &mut W) -> io::Result<()>;
pub async fn read_preface<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<()>;
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()>;
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Frame>;
```

Header encoding is big-endian. `read_frame` rejects unknown frame types, nonzero reserved bytes, `meta_len > MAX_META_LEN`, and `data_len > MAX_DATA_LEN` with `ErrorKind::InvalidData`.

- [ ] **Step 4: Run the focused verification**

Run: `cargo test -p remote-exec-proto port_tunnel --lib`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/Cargo.toml crates/remote-exec-proto/src/lib.rs crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/port_tunnel.rs
git commit -m "feat: add port tunnel frame codec"
```

---

### Task 2: Capability Reporting and Rejection

**Files:**
- Modify: `crates/remote-exec-host/src/state.rs`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets`, `cargo test -p remote-exec-broker --test mcp_forward_ports capability`, `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** TDD
Reason: Capability behavior is observable before the tunnel runtime exists and must prevent fallback to older daemons.

- [ ] **Step 1: Add failing capability tests**

Add broker test coverage that a daemon target returning `supports_port_forward: true` with missing or `1` `port_forward_protocol_version` fails `forward_ports` before the broker tries old routes. Add C++ route coverage asserting target-info JSON contains `"port_forward_protocol_version":2`.

Expected broker assertion shape:

```rust
assert!(message.contains("does not support port forward protocol version 2"));
```

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports capability`

Expected: FAIL because broker currently accepts `supports_port_forward` without the new protocol version.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

Expected: FAIL because C++ target-info does not emit the new field.

- [ ] **Step 3: Implement capability propagation**

Set `port_forward_protocol_version: 2` anywhere Rust daemon/local target info is built. Set `port_forward_protocol_version` to `2` in the C++ target-info JSON. Keep public `list_targets` schema unchanged; use the new field only internally for broker routing. In `forward_ports` remote side validation, require:

```rust
if !info.supports_port_forward || info.port_forward_protocol_version < 2 {
    return Err(anyhow::anyhow!(
        "target `{}` does not support port forward protocol version 2",
        target_name
    ));
}
```

Use a stable public error string containing:

```text
target `<name>` does not support port forward protocol version 2
```

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_assets`

Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports capability`

Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host crates/remote-exec-broker crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/tests
git commit -m "feat: require port tunnel capability"
```

---

### Task 3: Rust Host Tunnel Service

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-host port_tunnel --lib`

**Testing approach:** TDD
Reason: A Tokio duplex stream can drive the host tunnel service without HTTP or broker code.

- [ ] **Step 1: Add failing host tunnel tests**

Add `#[cfg(test)] mod port_tunnel_tests` to `crates/remote-exec-host/src/port_forward.rs` covering:

```rust
#[tokio::test]
async fn tunnel_binds_tcp_listener_and_releases_it_on_drop() {
    let state = test_state();
    let listen_endpoint = free_loopback_endpoint().await;
    let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(serve_tunnel(state.clone(), daemon_side));
    write_preface(&mut broker_side).await.unwrap();
    write_frame(&mut broker_side, &json_frame(FrameType::TcpListen, 1, serde_json::json!({ "endpoint": listen_endpoint }))).await.unwrap();
    let ok = read_frame(&mut broker_side).await.unwrap();
    assert_eq!(ok.frame_type, FrameType::TcpListenOk);
    drop(broker_side);
    wait_until_bindable(&listen_endpoint).await;
}

#[tokio::test]
async fn tunnel_tcp_connect_echoes_binary_data_full_duplex() {
    let state = test_state();
    let echo_endpoint = spawn_tcp_echo_server().await;
    let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(serve_tunnel(state, daemon_side));
    write_preface(&mut broker_side).await.unwrap();
    write_frame(&mut broker_side, &json_frame(FrameType::TcpConnect, 1, serde_json::json!({ "endpoint": echo_endpoint }))).await.unwrap();
    assert_eq!(read_frame(&mut broker_side).await.unwrap().frame_type, FrameType::TcpConnectOk);
    write_frame(&mut broker_side, &data_frame(FrameType::TcpData, 1, b"\0hello\xff".to_vec())).await.unwrap();
    let echoed = read_frame(&mut broker_side).await.unwrap();
    assert_eq!(echoed.frame_type, FrameType::TcpData);
    assert_eq!(echoed.data, b"\0hello\xff");
}

#[tokio::test]
async fn tunnel_udp_bind_relays_datagrams_from_two_peers() {
    let state = test_state();
    let endpoint = free_loopback_endpoint().await;
    let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
    tokio::spawn(serve_tunnel(state, daemon_side));
    write_preface(&mut broker_side).await.unwrap();
    write_frame(&mut broker_side, &json_frame(FrameType::UdpBind, 1, serde_json::json!({ "endpoint": endpoint }))).await.unwrap();
    let bind_ok = read_frame(&mut broker_side).await.unwrap();
    let bound_endpoint = endpoint_from_frame(&bind_ok);
    let peer_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let peer_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    peer_a.send_to(b"from-a", &bound_endpoint).await.unwrap();
    peer_b.send_to(b"from-b", &bound_endpoint).await.unwrap();
    let first = read_frame(&mut broker_side).await.unwrap();
    let second = read_frame(&mut broker_side).await.unwrap();
    assert_eq!(sorted_payloads([first.data, second.data]), vec![b"from-a".to_vec(), b"from-b".to_vec()]);
}
```

Use `tokio::io::duplex` for the tunnel transport. The broker side writes the preface and frames using `remote_exec_proto::port_tunnel`.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-host port_tunnel --lib`

Expected: FAIL because the host tunnel service does not exist.

- [ ] **Step 3: Implement host tunnel service**

Add:

```rust
pub async fn serve_tunnel<S>(state: Arc<AppState>, stream: S) -> Result<(), HostRpcError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static;
```

Implementation requirements:

- Read and validate `REPFWD1\n`.
- Split the stream and serialize all outgoing frames through one bounded `mpsc::Sender<Frame>` writer task.
- `TCP_LISTEN`: bind listener, send `TCP_LISTEN_OK { endpoint }`, spawn accept loop, daemon-created accepted stream IDs are even.
- `TCP_CONNECT`: connect to endpoint, send `TCP_CONNECT_OK`, spawn read/write pumps for that stream.
- `TCP_DATA`: write raw bytes to the matching TCP stream.
- `TCP_EOF` and `CLOSE`: half-close or close the matching resource.
- `UDP_BIND`: bind UDP socket, send `UDP_BIND_OK { endpoint }`, spawn receive loop.
- `UDP_DATAGRAM`: send raw datagram to metadata `peer`.
- Tunnel shutdown cancels all spawned tasks and closes all tunnel-owned sockets.
- Do not create or renew leases.

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-host port_tunnel --lib`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host/src/port_forward.rs
git commit -m "feat: serve port forward tunnels in host runtime"
```

---

### Task 4: Rust Daemon Upgrade Endpoint

**Files:**
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon/src/http/routes.rs`
- Modify: `crates/remote-exec-daemon/src/tls.rs`
- Modify: `crates/remote-exec-daemon/src/tls_enabled.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`

**Testing approach:** TDD
Reason: The daemon HTTP upgrade behavior is observable with raw HTTP requests before broker integration.

- [ ] **Step 1: Add failing daemon upgrade tests**

Add tests that:

- Send a valid HTTP/1.1 `POST /v1/port/tunnel` with `Connection: Upgrade`, `Upgrade: remote-exec-port-tunnel`, and `X-Remote-Exec-Port-Tunnel-Version: 1`; assert `101 Switching Protocols`.
- Send `HTTP/1.0` to the tunnel path; assert rejection.
- POST to `/v1/port/lease/renew`; assert `404 Not Found`.
- Open a tunnel, bind a TCP listener, drop the socket, and bind the same endpoint locally without waiting for lease expiry.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`

Expected: FAIL because `/v1/port/tunnel` is not routed and old routes still exist.

- [ ] **Step 3: Implement the Rust daemon upgrade route**

Add the Axum handler:

```rust
pub async fn tunnel(
    State(state): State<Arc<crate::AppState>>,
    request: axum::extract::Request,
) -> Result<Response, (StatusCode, Json<RpcErrorBody>)>
```

Validate `Connection`, `Upgrade`, and `X-Remote-Exec-Port-Tunnel-Version`. Return `101 Switching Protocols`, then use `tokio::spawn` and `hyper::upgrade::on(request)` to pass the upgraded stream into `remote_exec_host::port_forward::serve_tunnel`.

In both daemon HTTP serving files, change `serve_connection(io, service).await` to `serve_connection(io, service).with_upgrades().await`.

Route only `/v1/port/tunnel` for port forwarding and remove the old `/v1/port/listen`, `/v1/port/listen/accept`, `/v1/port/listen/close`, `/v1/port/lease/renew`, `/v1/port/connect`, `/v1/port/connection/read`, `/v1/port/connection/write`, `/v1/port/connection/close`, `/v1/port/udp/read`, and `/v1/port/udp/write` registrations from the router.

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src crates/remote-exec-daemon/tests/port_forward_rpc.rs
git commit -m "feat: add rust daemon port tunnel upgrade"
```

---

### Task 5: Broker Tunnel Client and Local Adapter

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-broker port_tunnel --lib`

**Testing approach:** TDD
Reason: The broker tunnel abstraction can be tested with local duplex streams and a stub HTTP upgrade server before rewriting public forwarding.

- [ ] **Step 1: Add failing broker tunnel tests**

Add unit tests that prove:

- `DaemonClient::port_tunnel()` sends the Upgrade headers and writes `REPFWD1\n` after `101`.
- Local tunnel creation uses `remote_exec_host::port_forward::serve_tunnel` over an in-process duplex stream.
- Missing or non-`101` daemon responses produce a `DaemonClientError` without falling back to old routes.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker port_tunnel --lib`

Expected: FAIL because no broker tunnel client exists.

- [ ] **Step 3: Implement broker tunnel creation**

Add a broker-owned tunnel handle that exposes:

```rust
async fn send(&self, frame: Frame) -> anyhow::Result<()>;
async fn recv(&self) -> anyhow::Result<Frame>;
async fn close_stream(&self, stream_id: u32) -> anyhow::Result<()>;
```

For remote targets, `DaemonClient::port_tunnel()` performs `POST /v1/port/tunnel`, validates `101`, calls `response.upgrade().await`, writes the preface, and returns an upgraded `AsyncRead + AsyncWrite` split behind the tunnel handle.

For `"local"`, create a Tokio duplex pair, spawn `remote_exec_host::port_forward::serve_tunnel(local_state, daemon_side)`, write the preface to the broker side, and return the same tunnel handle.

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-broker port_tunnel --lib`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/daemon_client.rs crates/remote-exec-broker/src/port_forward.rs
git commit -m "feat: open broker port tunnels"
```

---

### Task 6: Broker TCP Relay Over Tunnels

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `tests/e2e/multi_target.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports tcp`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports`

**Testing approach:** TDD
Reason: Public `forward_ports` TCP behavior already has integration seams and should remain unchanged while internals change.

- [ ] **Step 1: Strengthen failing TCP public tests**

Add or extend tests so a TCP forward sends a binary payload larger than one old read chunk in both directions. Keep the MCP request/response schema unchanged.

Add an e2e assertion that killing/dropping the broker releases a remote listener without waiting for lease expiry.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports tcp`

Expected: FAIL because broker still uses old routes and those routes are removed from the daemon router.

- [ ] **Step 3: Rewrite TCP forwarding**

Remove lease owners and per-connection HTTP reads/writes from the TCP path. For each TCP public forward:

- Open listen-side and connect-side tunnels.
- Send `TCP_LISTEN` to the listen tunnel and wait for `TCP_LISTEN_OK`.
- Spawn a background relay task that handles `TCP_ACCEPT` frames.
- For each accepted listen stream, allocate an odd connect-side stream ID and send `TCP_CONNECT`.
- After `TCP_CONNECT_OK`, pair stream IDs and relay `TCP_DATA`, `TCP_EOF`, and `CLOSE` frames in both directions through bounded channels.
- On public close, send `CLOSE` for the listener and all paired streams, then drop both tunnels.
- On unexpected tunnel failure, mark the public forward failed and clean up paired resources.

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports tcp`

Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports`

Expected: PASS for TCP forwarding cases.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs tests/e2e/multi_target.rs
git commit -m "feat: relay tcp forwards over tunnels"
```

---

### Task 7: Broker UDP Full-Duplex Relay

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `tests/e2e/multi_target.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports udp`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports`

**Testing approach:** TDD
Reason: The old UDP behavior was serialized request/reply; concurrent peer tests should fail until the relay is full duplex.

- [ ] **Step 1: Add failing full-duplex UDP tests**

Add a public UDP test with two independent UDP clients sending distinct payloads through the same listener before either reply is read. Assert replies return to the correct peers. Use the existing public `forward_ports` MCP request shape.

- [ ] **Step 2: Verify the red state**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports udp`

Expected: FAIL under the old serialized UDP behavior or while the tunnel relay is incomplete.

- [ ] **Step 3: Implement UDP tunnel relay**

For each UDP public forward:

- Open listen-side and connect-side tunnels.
- Send `UDP_BIND` to the listen side and return after `UDP_BIND_OK`.
- On `UDP_DATAGRAM` from listen side peer `P`, create or reuse one connect-side UDP connector stream for `P`.
- Send the datagram to the configured connect endpoint using metadata `peer = connect_endpoint`.
- On `UDP_DATAGRAM` from a connector stream, send the datagram back on the listen-side UDP stream using metadata `peer = P`.
- Use an idle timeout constant for per-peer connector streams.
- Close connector streams when idle, when public forward closes, or when either tunnel drops.

- [ ] **Step 4: Run focused verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports udp`

Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports`

Expected: PASS for UDP forwarding cases.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/port_forward.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs tests/e2e/multi_target.rs
git commit -m "feat: relay udp forwards full duplex"
```

---

### Task 8: C++ Frame Codec

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h`
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp`
- Create: `crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`

**Testing approach:** TDD
Reason: The C++ frame codec should match the Rust wire format and is testable without sockets.

- [ ] **Step 1: Add failing C++ frame tests**

Create C++17 tests that:

- Encode/decode a `TcpData` frame with embedded zero bytes.
- Reject unknown frame type.
- Reject metadata larger than 16 KiB.
- Reject payload larger than 256 KiB.
- Verify preface equals `REPFWD1\n`.

- [ ] **Step 2: Verify the red state**

Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`

Expected: FAIL because the source/header files and make target do not exist.

- [ ] **Step 3: Implement C++11 frame codec**

Add:

```cpp
enum class PortTunnelFrameType : unsigned char { Error = 1, Close = 2, TcpListen = 10, TcpListenOk = 11, TcpAccept = 12, TcpConnect = 13, TcpConnectOk = 14, TcpData = 15, TcpEof = 16, UdpBind = 30, UdpBindOk = 31, UdpDatagram = 32 };

struct PortTunnelFrame {
    PortTunnelFrameType type;
    unsigned char flags;
    uint32_t stream_id;
    std::string meta;
    std::vector<unsigned char> data;
};
```

Implement encode/decode helpers with big-endian length fields and explicit 16 KiB/256 KiB limits. Keep production code C++11-compatible.

- [ ] **Step 4: Run focused verification**

Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include/port_tunnel_frame.h crates/remote-exec-daemon-cpp/src/port_tunnel_frame.cpp crates/remote-exec-daemon-cpp/tests/test_port_tunnel_frame.cpp crates/remote-exec-daemon-cpp/Makefile
git commit -m "feat: add cpp port tunnel frame codec"
```

---

### Task 9: C++ Tunnel Engine and Upgrade Route

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/port_tunnel.h`
- Create: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_port_forward.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server_route_port_forward.h`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-server-streaming`

**Testing approach:** TDD
Reason: The C++ daemon upgrade and socket cleanup behavior is observable with socketpair-backed server tests.

- [ ] **Step 1: Add failing C++ tunnel server tests**

Add tests that:

- Send valid `POST /v1/port/tunnel` Upgrade and assert `101 Switching Protocols`.
- Bind a TCP listener through the tunnel, close the upgraded socket, and immediately bind the same endpoint.
- Establish a TCP echo flow over `TCP_CONNECT` and verify binary data both directions.
- Bind UDP and verify datagrams from two peers are emitted as independent `UDP_DATAGRAM` frames.
- POST to `/v1/port/lease/renew` and assert `404 Not Found`.

- [ ] **Step 2: Verify the red state**

Run: `make -C crates/remote-exec-daemon-cpp test-server-streaming`

Expected: FAIL because C++ Upgrade handling and tunnel engine do not exist.

- [ ] **Step 3: Implement C++ tunnel engine**

Implement a C++11-compatible tunnel runner that:

- Validates the request is HTTP/1.1, method `POST`, path `/v1/port/tunnel`, `Connection: Upgrade`, `Upgrade: remote-exec-port-tunnel`, and `X-Remote-Exec-Port-Tunnel-Version: 1`.
- Writes `101 Switching Protocols` with matching `Upgrade` and `Connection` headers.
- Owns the accepted socket after the upgrade and exits the normal persistent HTTP request loop.
- Runs read and write pumps guarded by a writer mutex.
- Implements `TCP_LISTEN`, `TCP_CONNECT`, `TCP_DATA`, `TCP_EOF`, `CLOSE`, `UDP_BIND`, and `UDP_DATAGRAM`.
- Tracks all listener, connection, and UDP sockets in a tunnel-owned collection and closes them when the upgraded socket closes.
- Removes lease renewal and old per-operation route dispatch from final route tables.

- [ ] **Step 4: Run focused verification**

Run: `make -C crates/remote-exec-daemon-cpp test-server-streaming`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/include crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp crates/remote-exec-daemon-cpp/Makefile
git commit -m "feat: add cpp daemon port tunnel"
```

---

### Task 10: Real Daemon Broker Coverage

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Modify: `tests/e2e/multi_target.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports`

**Testing approach:** characterization/integration test
Reason: The public schema stays unchanged; real daemon tests prove Rust and C++ daemons work through the new internal protocol.

- [ ] **Step 1: Replace old helper assumptions in tests**

Update test helpers so they no longer call old daemon `/v1/port/listen` routes directly. Use raw `TcpListener`/`UdpSocket` test servers and public MCP `forward_ports` requests to verify behavior.

- [ ] **Step 2: Run focused real-daemon verification**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`

Expected: PASS.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports`

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/remote-exec-broker/tests tests/e2e/multi_target.rs
git commit -m "test: cover real daemon port tunnels"
```

---

### Task 11: Remove Old Protocol Plumbing

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-host/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon-cpp/include/port_forward.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_port_forward.cpp`
- Test/Verify: static scans plus focused Rust/C++ tests

**Testing approach:** existing tests + targeted verification
Reason: This is cleanup after tunnel behavior is covered; static scans prove lease and old-route code paths are gone.

- [ ] **Step 1: Delete obsolete structs, helpers, and constants**

Remove unused old internal RPC items:

- `PortForwardLease`
- `PortLeaseRenewRequest`
- old per-operation listen/connect/read/write/close structs if no code uses them
- `FORWARD_LEASE_TTL_MS`
- `FORWARD_LEASE_RENEW_INTERVAL_MS`
- lease renewal stores and sweepers
- C++ lease-expiry tests and route handlers

- [ ] **Step 2: Run static scans**

Run:

```bash
rg -n "/v1/port/lease/renew|PortForwardLease|PortLeaseRenewRequest|FORWARD_LEASE_RENEW_INTERVAL|connection/read|connection/write|/v1/port/udp/read|/v1/port/udp/write" crates tests README.md skills configs
```

Expected: No production code references. Documentation references only appear after Task 12 if they describe removal rather than active behavior.

- [ ] **Step 3: Run focused verification**

Run: `cargo test -p remote-exec-proto --lib`

Expected: PASS.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`

Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`

Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates tests
git commit -m "refactor: remove legacy port forward rpc"
```

---

### Task 12: Documentation

**Files:**
- Modify: `README.md`
- Modify: `configs/broker.example.toml`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `rg -n "lease renewal stops|lease|/v1/port/lease/renew|connection/read|connection/write" README.md configs skills crates/remote-exec-daemon-cpp/README.md`

**Testing approach:** no new tests needed
Reason: These are prose updates; static scans verify stale active-behavior wording is gone.

- [ ] **Step 1: Update lifecycle wording**

Update docs so `forward_ports` says daemon-side resources are owned by the internal HTTP/1.1 Upgrade tunnel and are reclaimed when the tunnel closes. Remove wording that says broker crash cleanup waits for lease renewal to stop.

- [ ] **Step 2: Update C++ daemon docs**

Document that the C++ daemon supports the same tunnel-based port-forward contract, uses HTTP/1.1 Upgrade, and no longer exposes lease-renewed old port-forward routes.

- [ ] **Step 3: Run documentation scans**

Run:

```bash
rg -n "lease renewal stops|lease renew|/v1/port/lease/renew|connection/read|connection/write" README.md configs skills crates/remote-exec-daemon-cpp/README.md
```

Expected: No stale active-behavior references.

- [ ] **Step 4: Commit**

```bash
git add README.md configs/broker.example.toml crates/remote-exec-daemon-cpp/README.md skills/using-remote-exec-mcp/SKILL.md
git commit -m "docs: document port tunnel forwarding"
```

---

### Task 13: Full Verification Gate

**Files:**
- Modify: none if the gate passes
- Test/Verify: full Rust workspace and C++ daemon checks

**Testing approach:** existing tests + targeted verification
Reason: The implementation changes shared protocol, broker runtime, Rust daemon, C++ daemon, and docs.

- [ ] **Step 1: Run focused Rust port-forward suites**

Run:

```bash
cargo test -p remote-exec-proto port_tunnel --lib
cargo test -p remote-exec-host port_tunnel --lib
cargo test -p remote-exec-daemon --test port_forward_rpc
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
cargo test -p remote-exec-broker --test multi_target -- --nocapture forward_ports
```

Expected: PASS.

- [ ] **Step 2: Run C++ daemon checks**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame
make -C crates/remote-exec-daemon-cpp test-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: PASS.

- [ ] **Step 3: Run full workspace gate**

Run:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
git diff --check
git status --short
```

Expected: PASS for commands and only intentional untracked/modified files before final commit.

- [ ] **Step 4: Commit any verification-only fixes**

If formatting or lint fixes changed files:

```bash
git add <changed-files>
git commit -m "chore: finalize port tunnel forwarding"
```

If no files changed, do not create an empty commit.
