# Port Forward Structural Cleanups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve the still-valid structural review findings #22, #23, #25, #26, #27, #28, and the still-relevant boundary part of #29 without changing the port tunnel protocol or public tool behavior.

**Architecture:** The Rust broker keeps the existing v4 tunnel behavior but moves protocol-specific opening details into a small descriptor, centralizes forward runtime construction, and turns the TCP bridge select loop into a small event demux that delegates frame behavior to helper handlers. The C++ daemon splits write-side frame sending and transport-owned stream/socket maps out of `PortTunnelConnection`, then decomposes config parsing into section helpers. Rust embedded daemon config becomes a thin wrapper around host config, and host RPC error-to-wire conversion moves behind a shared method instead of being rebuilt by each caller.

**Tech Stack:** Rust 2024 with Tokio and existing broker/host port-forward tests; C++ daemon using existing C++11/XP-compatible primitives and the current GNU make/BSD make source inventory.

---

### Task 1: Save This Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-10-port-forward-structural-cleanups.md`
- Test/Verify: `git status --short`

**Testing approach:** no new tests needed
Reason: This task only adds the implementation plan artifact.

- [ ] **Step 1: Add this plan file.**

The file is this document.

- [ ] **Step 2: Verify the plan is the only intended tracked change.**

Run: `git status --short`
Expected: `docs/superpowers/plans/2026-05-10-port-forward-structural-cleanups.md` appears as untracked or staged; pre-existing unrelated files such as `.swp` remain untouched.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-10-port-forward-structural-cleanups.md
git commit -m "docs: plan port forward structural cleanups"
```

### Task 2: Unify Broker Forward Opening And Runtime Construction

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** existing tests + targeted verification
Reason: This is behavior-preserving broker structure cleanup for forward creation; current e2e-style broker port-forward tests cover TCP/UDP open, close, reconnect, and error handling.

- [ ] **Step 1: Add a protocol descriptor and runtime constructor.**

Add this shape near the existing forward structs:

```rust
#[derive(Clone, Copy)]
struct ForwardOpenKind {
    protocol: PublicForwardPortProtocol,
    listen_frame_type: FrameType,
    listen_ok_frame_type: FrameType,
    noun: &'static str,
}

impl ForwardOpenKind {
    fn for_protocol(protocol: PublicForwardPortProtocol) -> Self { ... }
}

struct ForwardRuntimeParts { ... }

impl ForwardRuntime {
    fn new(parts: ForwardRuntimeParts) -> Self { ... }
}
```

- [ ] **Step 2: Replace `open_tcp_forward` and `open_udp_forward` with one `open_protocol_forward`.**

The helper must:
- generate one `forward_id`
- open the listen tunnel and connect tunnel with `kind.protocol`
- send `kind.listen_frame_type`
- wait for `kind.listen_ok_frame_type`
- create `ListenSessionControl`
- build `ForwardRuntime` through `ForwardRuntime::new`
- return the same `OpenedForward` shape and public entry fields as before

- [ ] **Step 3: Route `open_forward` through the unified helper.**

Keep existing endpoint normalization and public error behavior.

- [ ] **Step 4: Run focused verification.**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/supervisor.rs
git commit -m "refactor: unify broker forward opening"
```

### Task 3: Split Broker TCP Bridge Frame Handling

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** existing tests + targeted verification
Reason: The change is a control-flow refactor inside the TCP bridge; existing tests exercise the accept, pending flush, EOF, close, error, pressure, and recovery paths.

- [ ] **Step 1: Add per-side tunnel event helpers.**

Add helpers with this interface:

```rust
async fn handle_listen_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_ids: &mut StreamIdAllocator,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> { ... }

async fn handle_connect_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> { ... }
```

- [ ] **Step 2: Add frame-specific helpers for the large match arms.**

Extract listen-side helpers:
- `handle_listen_tcp_accept`
- `handle_listen_tcp_data`
- `handle_listen_tcp_eof`
- `handle_listen_close`
- `handle_listen_error`

Extract connect-side helpers:
- `handle_connect_tcp_connect_ok`
- `handle_connect_error`
- `handle_connect_tcp_data`
- `handle_connect_tcp_eof`
- `handle_connect_close`

- [ ] **Step 3: Shrink `run_tcp_forward_epoch` to select-only demux.**

The loop should only select cancellation, listen event, and connect event, then return when a helper returns `Some(ForwardLoopControl)`.

- [ ] **Step 4: Run focused verification.**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: all tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/tcp_bridge.rs
git commit -m "refactor: split broker tcp bridge handlers"
```

### Task 4: Extract C++ Tunnel Sender And Transport-Owned State

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** existing tests + targeted verification
Reason: The C++ refactor touches v4 tunnel I/O, TCP stream maps, UDP socket maps, and close behavior. The server streaming tests cover the port tunnel upgrade and stream behavior; final POSIX check runs later.

- [ ] **Step 1: Add `PortTunnelSender`.**

Move writer mutex and queued-byte accounting behind a helper:

```cpp
class PortTunnelSender {
public:
    PortTunnelSender(SOCKET client, const std::shared_ptr<PortTunnelService>& service);
    bool closed() const;
    void mark_closed();
    void send_frame(const PortTunnelFrame& frame);
    bool send_data_frame_or_limit_error(PortTunnelConnection& connection, const PortTunnelFrame& frame);
    bool send_data_frame_or_drop_on_limit(PortTunnelConnection& connection, const PortTunnelFrame& frame);
private:
    bool try_reserve_data_frame(const PortTunnelFrame& frame, unsigned long* charge_value);
    void release_data_frame_reservation(unsigned long charge_value);
    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    BasicMutex writer_mutex_;
    std::atomic<bool> closed_;
    std::atomic<unsigned long> queued_bytes_;
};
```

- [ ] **Step 2: Add `TransportOwnedStreams`.**

Move the transport-owned TCP and UDP maps behind a helper:

```cpp
class TransportOwnedStreams {
public:
    void insert_tcp(uint32_t stream_id, const std::shared_ptr<TunnelTcpStream>& stream);
    std::shared_ptr<TunnelTcpStream> get_tcp(uint32_t stream_id);
    std::shared_ptr<TunnelTcpStream> remove_tcp(uint32_t stream_id);
    void insert_udp(uint32_t stream_id, const std::shared_ptr<TunnelUdpSocket>& socket_value);
    std::shared_ptr<TunnelUdpSocket> get_udp(uint32_t stream_id);
    std::shared_ptr<TunnelUdpSocket> remove_udp(uint32_t stream_id);
    void drain(std::vector<std::shared_ptr<TunnelTcpStream> >* tcp_streams,
               std::vector<std::shared_ptr<TunnelUdpSocket> >* udp_sockets);
private:
    BasicMutex mutex_;
    std::map<uint32_t, std::shared_ptr<TunnelTcpStream> > tcp_streams_;
    std::map<uint32_t, std::shared_ptr<TunnelUdpSocket> > udp_sockets_;
};
```

- [ ] **Step 3: Update `PortTunnelConnection` to delegate sender and map operations.**

Keep the public methods `send_frame`, `send_data_frame_or_limit_error`, `send_data_frame_or_drop_on_limit`, and `closed()` as thin compatibility wrappers so the rest of the tunnel code changes minimally.

- [ ] **Step 4: Run focused verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: test binary builds and passes.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h \
  crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp \
  crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp
git commit -m "refactor: split cpp port tunnel connection state"
```

### Task 5: Split C++ Config Loader Helpers

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-config`

**Testing approach:** existing tests + targeted verification
Reason: Existing config tests cover required fields, defaults, invalid limits, auth validation, yield-time bounds, and sandbox lists. This task preserves those behaviors while reducing `load_config`.

- [ ] **Step 1: Extract file parsing and section readers.**

Add helpers:

```cpp
typedef std::map<std::string, std::string> ConfigValues;

static ConfigValues read_config_values(const std::string& path);
static std::string read_http_auth_bearer_token(const ConfigValues& values);
static PortForwardLimitConfig read_port_forward_limits(const ConfigValues& values);
static YieldTimeConfig read_yield_time_config(const ConfigValues& values);
static FilesystemSandbox read_sandbox(const ConfigValues& values);
static void validate_daemon_config(const DaemonConfig& config);
static void validate_port_forward_limits(const PortForwardLimitConfig& limits);
```

- [ ] **Step 2: Rewrite `load_config` as orchestration.**

`load_config` should read values, assign core fields, call section helpers, validate, then return `DaemonConfig`.

- [ ] **Step 3: Run focused verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
Expected: config tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/config.cpp
git commit -m "refactor: split cpp config loading"
```

### Task 6: Deduplicate Embedded Daemon Config

**Files:**
- Modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test health`

**Testing approach:** existing tests + targeted verification
Reason: The change is internal config construction; daemon health tests compile and instantiate daemon config paths.

- [ ] **Step 1: Change `EmbeddedDaemonConfig` to wrap `EmbeddedHostConfig`.**

Use:

```rust
pub struct EmbeddedDaemonConfig {
    pub host: EmbeddedHostConfig,
}
```

- [ ] **Step 2: Update conversions.**

`into_host_config` should return `self.host`. `From<EmbeddedHostConfig> for EmbeddedDaemonConfig` should wrap `host: value`. `into_daemon_config` should destructure the host config once and fill daemon-only defaults.

- [ ] **Step 3: Run focused verification.**

Run: `cargo test -p remote-exec-daemon --test health`
Expected: health tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-daemon/src/config/mod.rs
git commit -m "refactor: wrap embedded daemon host config"
```

### Task 7: Centralize Host RPC Error Wire Conversion

**Files:**
- Modify: `crates/remote-exec-host/src/error.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Modify: `crates/remote-exec-daemon/src/rpc_error.rs`
- Test/Verify: `cargo test -p remote-exec-broker local_backend`

**Testing approach:** existing tests + targeted verification
Reason: This is boundary-polish for error mapping; existing local backend tests assert server status/code/message preservation.

- [ ] **Step 1: Add a host-owned conversion method.**

Add to `HostRpcError`:

```rust
pub fn into_rpc_parts(self) -> (u16, remote_exec_proto::rpc::RpcErrorBody) { ... }
```

- [ ] **Step 2: Use the method in broker and daemon adapters.**

The broker should construct `DaemonClientError::Rpc` from the returned status/body. The daemon should construct `(StatusCode, Json<RpcErrorBody>)` from the same returned parts.

- [ ] **Step 3: Run focused verification.**

Run: `cargo test -p remote-exec-broker local_backend`
Expected: local backend tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-host/src/error.rs \
  crates/remote-exec-broker/src/local_backend.rs \
  crates/remote-exec-daemon/src/rpc_error.rs
git commit -m "refactor: centralize host rpc error mapping"
```

### Task 8: Final Focused Verification

**Files:**
- Test/Verify: Rust broker/daemon/host and C++ POSIX checks

**Testing approach:** existing tests + targeted verification
Reason: The work spans broker forwarding, daemon config, host error mapping, and C++ tunnel internals.

- [ ] **Step 1: Run final focused verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-daemon --test health
cargo test -p remote-exec-broker local_backend
cargo check -p remote-exec-host -p remote-exec-broker -p remote-exec-daemon
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: all commands pass.
