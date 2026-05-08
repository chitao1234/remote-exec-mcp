# Port Forward v4 Hardening Design

## Context

The current `forward_ports` implementation is broker-mediated. The public
broker owns `forward_id` values and routes traffic between a listen side and a
connect side through daemon-private HTTP/1.1 Upgrade tunnels. The Rust daemon
and the C++ daemon both implement the daemon-side tunnel protocol.

The existing design correctly preserves target isolation and keeps public
session IDs separate from daemon-local state, but the live implementation has
several high-impact reliability and operability problems:

- tunnel tasks and upgraded streams do not have explicit lifecycle ownership
- connect-side reconnect is weaker than listen-side reconnect
- public status cannot describe degraded or reconnecting forwards
- broker and daemon forwarding resources are mostly unbounded
- daemon session semantics differ between Rust and C++
- open, connect, close, and handshake paths do not consistently use explicit
  operation timeouts
- flow control is frame-count based instead of byte-budget based
- stream ID counters can wrap into live IDs on long-lived forwards

This design introduces forwarding protocol v4 to make those fixes first-class
instead of layering more behavior onto the existing v3 forwarding capability and
tunnel header version 2.

## Goals

- Preserve broker ownership of public `forward_id` values.
- Preserve broker-mediated data flow for this hardening phase.
- Preserve per-target isolation: a forward opened on one side must not be
  usable on another side.
- Introduce `port_forward_protocol_version = 4` for hardened forwarding.
- Align the daemon tunnel upgrade header with the forwarding capability by
  requiring `x-remote-exec-port-tunnel-version: 4` for v4 tunnels.
- Expose reconnecting and degraded state through additive public result fields.
- Add explicit tunnel close and task shutdown semantics.
- Apply symmetric reconnect budgets to listen-side and connect-side transport
  loss.
- Add broker and daemon resource limits with stable errors.
- Add operation timeouts and byte-aware backpressure.
- Prevent stream ID wraparound from colliding with live streams.
- Keep Rust daemon and C++ daemon behavior aligned.

## Non-Goals

- No direct daemon-to-daemon data plane in this change.
- No per-call approval flow or interactive sandbox escalation.
- No attempt to preserve active TCP streams across tunnel reconnect.
- No attempt to preserve UDP per-peer connector state across connect-side
  reconnect.
- No public breaking removal of existing `ForwardPortEntry` fields.

## Versioning And Capability

Targets that support the hardened behavior report:

```text
supports_port_forward = true
port_forward_protocol_version = 4
```

The broker uses v4 behavior only when every remote side involved in a forward
reports `port_forward_protocol_version >= 4`. Side `"local"` is implemented by
the embedded host runtime and must support v4 internally.

The daemon HTTP/1.1 Upgrade request for v4 forwarding must include:

```http
Connection: Upgrade
Upgrade: remote-exec-port-tunnel
x-remote-exec-port-tunnel-version: 4
Content-Length: 0
```

The broker should reject remote targets below v4 for the hardened path with a
stable error message:

```text
target `<name>` does not support port forward protocol version 4
```

Existing v3 code can remain temporarily for compatibility during migration, but
the v4 implementation is the target design. New reconnect, quota, lifecycle,
and observability semantics are guaranteed only on v4.

## Public API Additions

The public `ForwardPortEntry` keeps existing fields:

- `forward_id`
- `listen_side`
- `listen_endpoint`
- `connect_side`
- `connect_endpoint`
- `protocol`
- `status`
- `last_error`

Add fields:

```rust
pub phase: ForwardPortPhase,
pub listen_state: ForwardPortSideState,
pub connect_state: ForwardPortSideState,
pub active_tcp_streams: u64,
pub dropped_tcp_streams: u64,
pub dropped_udp_datagrams: u64,
pub reconnect_attempts: u64,
pub last_reconnect_at: Option<String>,
pub limits: ForwardPortLimitSummary,
```

`phase` values:

```text
opening
ready
reconnecting
draining
closing
closed
failed
```

`ForwardPortSideState` contains:

```rust
pub side: String,
pub role: ForwardPortSideRole, // listen | connect
pub generation: u64,
pub health: ForwardPortSideHealth,
pub last_error: Option<String>,
```

`ForwardPortSideHealth` values:

```text
starting
ready
reconnecting
degraded
closed
failed
```

Legacy `status` mapping remains:

- `status = open` when `phase` is `opening`, `ready`, `reconnecting`, or
  `draining`
- `status = closed` when `phase` is `closed`
- `status = failed` when `phase` is `failed`

Text output should surface `phase` when it is more specific than the legacy
status. For example, a reconnecting forward should render as
`tcp, open, reconnecting` rather than only `tcp, open`.

## Tunnel Protocol v4

The v4 frame envelope remains compatible with the existing binary frame shape:

- frame type byte
- flags byte
- reserved bytes
- stream ID
- metadata length
- data length
- metadata bytes
- data bytes

The v4 protocol adds generation-aware control semantics. Control frames use
`stream_id = 0`. Data-plane frames use nonzero stream IDs within the currently
attached tunnel generation.

### New Control Frames

Add these frame types:

- `TunnelOpen`
- `TunnelReady`
- `TunnelClose`
- `TunnelClosed`
- `TunnelHeartbeat`
- `TunnelHeartbeatAck`
- `ForwardRecovering`
- `ForwardRecovered`

`TunnelOpen` metadata:

```json
{
  "forward_id": "fwd_...",
  "role": "listen|connect",
  "side": "builder-a",
  "generation": 1,
  "protocol": "tcp|udp",
  "resume_session_id": "sess_..."
}
```

`resume_session_id` is present only for listen-side resume. A fresh listen
tunnel omits it.

`TunnelReady` metadata:

```json
{
  "generation": 1,
  "session_id": "sess_...",
  "resume_timeout_ms": 10000,
  "limits": {
    "max_active_tcp_streams": 256,
    "max_udp_peers": 256,
    "max_queued_bytes": 8388608
  }
}
```

`session_id` and `resume_timeout_ms` are included only for listen-side tunnels
with retained daemon resources.

`ForwardRecovering` metadata:

```json
{
  "forward_id": "fwd_...",
  "role": "listen|connect",
  "old_generation": 1,
  "reason": "transport_loss|heartbeat_timeout|backpressure|operator_close"
}
```

`ForwardRecovered` metadata:

```json
{
  "forward_id": "fwd_...",
  "role": "listen|connect",
  "generation": 2
}
```

`TunnelClose` metadata:

```json
{
  "forward_id": "fwd_...",
  "generation": 2,
  "reason": "operator_close|open_failed|shutdown|failed"
}
```

`TunnelClosed` echoes `forward_id`, `generation`, and `reason`.

### Error Frames

Error frames keep stable structured metadata:

```json
{
  "code": "port_forward_limit_exceeded",
  "message": "target `builder-a` has reached the port forward limit",
  "fatal": true,
  "generation": 2
}
```

`fatal = true` means the tunnel generation or whole forward must stop. Stream
errors use `fatal = false` unless they indicate protocol corruption.

## Broker Architecture

### Forward Supervisor

Replace the current single task loop with a supervisor that owns:

- public `forward_id`
- protocol and endpoint configuration
- public phase and counters
- listen-side runtime state
- connect-side runtime state
- cancellation token
- reconnect policy
- effective limits

Each side runtime tracks:

- role
- side handle
- current tunnel generation
- current tunnel handle
- current health
- last error
- reconnect attempts
- active stream or connector state tied to that generation

The supervisor is responsible for phase transitions:

```text
opening -> ready
ready -> reconnecting
reconnecting -> ready
ready -> closing -> closed
ready/reconnecting/opening -> failed
```

### PortTunnel Ownership

Broker `PortTunnel` becomes an explicitly owned resource:

- owns reader task handle
- owns writer task handle
- owns cancellation token
- owns a byte-aware outbound queue
- exposes `close(reason)` for graceful close
- exposes `abort()` for forced transport teardown
- joins reader/writer tasks with a short timeout

Dropping `PortTunnel` must not silently leave an upgraded stream alive. Drop may
call `abort()` as a best-effort fallback, but normal forward close uses explicit
`close(reason)` and waits for acknowledgement when possible.

### Open Flow

For each forward:

1. Validate endpoints and sides.
2. Check broker-side quotas.
3. Open listen tunnel with `TunnelOpen`.
4. Receive `TunnelReady`.
5. Send `TcpListen` or `UdpBind`.
6. Receive `TcpListenOk` or `UdpBindOk`.
7. Open connect tunnel with `TunnelOpen`.
8. Receive `TunnelReady`.
9. Register the forward in the store.
10. Start the supervisor in `ready` phase.

Failed initialization remains all-or-nothing. Any listener or bind created by a
failed open is cleaned up using the same v4 close path as normal close.

### Close Flow

Explicit close transitions the public forward to `closing` and cancels new
work. The broker then:

1. Sends `TunnelClose` to both sides where a tunnel exists.
2. Sends listener or bind `Close` for retained listen resources.
3. Waits for `TunnelClosed` or stream `Close` acknowledgement within the close
   timeout.
4. Aborts any tunnel that does not acknowledge within the timeout.
5. Marks the forward `closed` only after cleanup is confirmed or after the
   retained resource is known to be gone.

If cleanup cannot be confirmed, the forward is marked `failed` and remains
listed with `last_error`, preserving the current inspect-and-retry behavior.

## Reconnect Model

Listen-side and connect-side recovery use a shared reconnect policy:

```rust
pub struct PortForwardReconnectPolicy {
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
    pub jitter_ratio: f32,
    pub attempt_timeout_ms: u64,
    pub total_timeout_ms: u64,
    pub max_attempts: u32,
}
```

Defaults:

- initial backoff: 50 ms
- max backoff: 500 ms
- jitter: 20 percent
- attempt timeout: 2 seconds
- total timeout: daemon resume timeout minus 250 ms for listen-side resume,
  10 seconds for connect-side reopen
- max attempts: unlimited within the total timeout

### Listen-Side Loss

On retryable listen-side transport loss:

1. Mark phase `reconnecting`.
2. Mark listen state `reconnecting`.
3. Increment reconnect counters.
4. Close active TCP streams and drop UDP per-peer connector state.
5. Reopen a tunnel with `TunnelOpen` and `resume_session_id`.
6. On `TunnelReady`, bump listen generation.
7. Reopen connect-side tunnel to avoid mixing generations.
8. Mark both sides `ready` and phase `ready`.

The daemon retains only listen-side TCP listeners or UDP binds during the
resume window. Active accepted TCP streams are not preserved.

### Connect-Side Loss

On retryable connect-side transport loss:

1. Mark phase `reconnecting`.
2. Mark connect state `reconnecting`.
3. Increment reconnect counters.
4. Close active TCP streams on the listen side.
5. Drop UDP per-peer connector state.
6. Reopen a fresh connect tunnel with `TunnelOpen`.
7. On `TunnelReady`, bump connect generation.
8. Mark connect side `ready` and phase `ready`.

Connect-side recovery does not require daemon-retained session state.

### Failure Conditions

A forward fails when:

- reconnect total timeout expires
- max attempts is exceeded
- daemon returns a fatal tunnel error
- daemon returns an unrecoverable protocol error
- close cleanup cannot be confirmed
- a quota violation requires terminating the forward

The public entry keeps the final side states and `last_error`.

## Daemon Architecture

### Close Modes

The daemon must distinguish:

- explicit graceful close
- retryable detach
- terminal protocol failure

Only retryable detach retains listen-side resources. Explicit close and terminal
failure release retained listeners, UDP binds, active TCP streams, and queued
frames.

### Rust Host Runtime

The Rust retained session model should match the protocol:

- support a map of retained TCP listeners by stream ID
- support a map of retained UDP binds by stream ID
- keep per-session generation metadata
- reject duplicate listener or bind stream IDs in a generation
- reject frames whose generation does not match the attached tunnel
- close retained resources deterministically on explicit close or terminal
  failure

The broker currently opens one listener or bind per session. The Rust daemon
should still enforce protocol invariants instead of relying on broker behavior.

### C++ Daemon

The C++ daemon should keep behavior aligned with Rust:

- same v4 upgrade header
- same control frames
- same close modes
- same error codes
- same retained listen-side semantics
- same generation checks
- same quota categories

C++ may continue using native threads, including Windows XP-compatible code
paths, but it must enforce worker limits before creating new listener, read, or
expiry threads.

## Resource Limits

Add config-backed limits with conservative defaults.

### Broker Limits

- `max_open_forwards_total`
- `max_forwards_per_side_pair`
- `max_active_tcp_streams_per_forward`
- `max_pending_tcp_bytes_per_stream`
- `max_pending_tcp_bytes_per_forward`
- `max_udp_peers_per_forward`
- `max_tunnel_queued_bytes`
- `max_reconnecting_forwards`

### Daemon Limits

- `max_tunnel_connections`
- `max_retained_sessions`
- `max_retained_listeners`
- `max_udp_binds`
- `max_active_tcp_streams`
- `max_tunnel_queued_bytes`
- `max_forward_worker_threads` for C++ daemon

Limit failures use stable error codes:

- `port_forward_limit_exceeded`
- `port_tunnel_limit_exceeded`
- `port_forward_backpressure_exceeded`
- `port_forward_reconnect_budget_exhausted`

Opening a forward that exceeds a limit fails before registering a public
forward whenever possible. Runtime limit failures update the public forward to
`failed` only when the entire forward must be stopped; stream-local pressure
closes or drops the affected stream/datagram first.

## Timeouts

Add explicit timeouts for:

- daemon tunnel upgrade
- tunnel preface exchange
- `TunnelOpen` / `TunnelReady`
- session resume
- listener bind acknowledgement
- UDP bind acknowledgement
- TCP connect acknowledgement
- tunnel close acknowledgement
- reconnect attempt
- graceful task shutdown

Every timeout should produce a stable error context that identifies the side,
role, forward ID, generation, and operation.

## Backpressure

The current queue model is frame-count based. v4 should account for bytes.

Each tunnel has a queued-byte budget. A frame is charged by:

```text
frame_overhead + metadata_length + data_length
```

Control frames have priority over data frames. Data pressure is handled as:

- TCP pending before connect acknowledgement is capped per stream and per
  forward.
- TCP data pressure after connect readiness closes the affected TCP stream
  before failing the whole forward.
- UDP pressure drops datagrams and increments `dropped_udp_datagrams`.
- Sustained control-channel pressure fails the tunnel generation and triggers
  reconnect or forward failure.

Public counters expose dropped TCP streams and UDP datagrams.

## Stream ID And Generation Semantics

Each tunnel generation has its own stream ID namespace. Odd and even allocation
rules can remain as they are today, but IDs must not silently wrap into active
streams.

When a stream ID allocator approaches exhaustion:

1. Stop accepting new streams on that generation.
2. Drain or close active streams.
3. Open a new tunnel generation.
4. Continue forwarding on the new generation.

Frames for unknown streams in the current generation are stream-level errors
unless the frame is an allowed late close/eof. Frames from old generations are
ignored after debug logging, except terminal protocol-corruption frames.

## Public Store Behavior

The broker store keeps entries after runtime failure so callers can inspect
them. Listing supports the existing filters plus the additive fields.

Close is idempotent for already closed entries only if the entry remains in the
store during a short terminal retention window. Unknown IDs still return
`unknown forward_id` after the entry is removed. The default behavior remains
that successful close removes the live record and returns a closed entry in the
tool response.

## Documentation Updates

Update:

- `README.md`
- `configs/broker.example.toml`
- `configs/daemon.example.toml`
- `crates/remote-exec-daemon-cpp/README.md`
- `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`
- `skills/using-remote-exec-mcp/SKILL.md`

Documentation must call out:

- v4 capability and tunnel header alignment
- additive public forwarding state fields
- reconnect semantics and data-loss boundaries
- broker and daemon limit settings
- `"local"` network access and network pivot implications
- C++ daemon worker limits

## Testing Strategy

### Broker Unit And Integration Tests

- `forward_ports` rejects remote targets below protocol v4 for hardened path.
- `forward_ports list` exposes `phase`, side health, counters, and limits.
- listen-side reconnect sets `phase = reconnecting` and returns to `ready`.
- connect-side reconnect retries within budget and returns to `ready`.
- reconnect budget exhaustion marks the forward `failed`.
- active TCP streams close and `dropped_tcp_streams` increments after either
  side tunnel loss.
- UDP datagrams dropped during pressure increment `dropped_udp_datagrams`.
- failed open cleanup uses explicit close and leaves no listed forward.
- close failure marks forward failed and preserves inspectable `last_error`.
- byte-aware backpressure closes affected streams before failing forwards.
- stream ID exhaustion rotates generation instead of wrapping.

### Rust Daemon Tests

- v4 upgrade requires `x-remote-exec-port-tunnel-version: 4`.
- `TunnelOpen` / `TunnelReady` round trips.
- explicit `TunnelClose` releases retained resources immediately.
- retryable detach retains listen-side resources until resume timeout.
- terminal protocol failure releases retained resources immediately.
- duplicate stream IDs are rejected.
- old-generation frames are ignored or rejected as specified.
- daemon resource limits return stable error codes.

### C++ Daemon Tests

- v4 upgrade and control frames match Rust behavior.
- worker limits reject new forwarding work before thread exhaustion.
- explicit close, retryable detach, and terminal failure match Rust behavior.
- TCP and UDP retained listen-side resume behavior matches Rust behavior.
- stable error codes match Rust behavior.

### End-To-End Tests

- local-to-local TCP and UDP forwarding.
- local-to-Rust-daemon TCP and UDP forwarding.
- Rust-daemon-to-local TCP and UDP forwarding.
- Rust-daemon-to-Rust-daemon forwarding through broker.
- C++ daemon forwarding parity for TCP, UDP, reconnect, close, and limits.
- broker crash releases daemon-retained listeners after resume timeout.
- daemon restart fails existing public forwards and allows reopening.

## Rollout Plan

1. Add v4 protocol constants and schemas.
2. Add additive public result fields.
3. Implement broker `PortTunnel` explicit lifecycle ownership.
4. Implement broker supervisor phase and side-state model.
5. Implement v4 Rust daemon control frames and close modes.
6. Add symmetric reconnect budgets.
7. Add byte-aware backpressure and counters.
8. Add broker and daemon limits.
9. Add C++ daemon v4 parity.
10. Update docs and skill guidance.
11. Remove or quarantine v3-only forwarding tests once v4 coverage is stable.

## Compatibility

Existing clients that only read `status` can continue to work. They will see
`open` during reconnect because v4 preserves the legacy mapping.

New clients should read `phase` and side health for accurate readiness.

During migration, brokers may keep v3 forwarding available only where existing
tests or deployments require it. v4 is the documented behavior for hardened
forwarding. Once all supported daemons report v4, v3-specific reconnect code can
be removed in a follow-up cleanup.

## Open Decisions Resolved By This Spec

- Direct daemon-to-daemon forwarding is deferred.
- The public API uses additive fields instead of breaking the existing
  `ForwardPortEntry.status` contract.
- `x-remote-exec-port-tunnel-version` is aligned to `4`.
- Active TCP streams are closed, not resumed, across tunnel reconnect.
- UDP per-peer connector state is dropped, not resumed, across connect-side
  reconnect.
- Quotas are required at both broker and daemon layers.
