# Port Forward Refactor Design

**Date:** 2026-05-07

## Goal

Refactor the internal port forwarding implementation so `forward_ports` keeps
its existing public MCP schema while the runtime ownership model becomes
coherent, teardown semantics become explicit, and the broker stops carrying a
large share of daemon-side session semantics.

The target is not a new feature. The target is a safer and clearer internal
design that reduces future change cost across:

- `remote-exec-broker`
- `remote-exec-host`
- `remote-exec-daemon`
- `remote-exec-daemon-cpp`
- port-forward-specific tests and support code

## Constraints

- Keep the public `forward_ports` tool name, request schema, and response schema
  stable.
- Preserve broker ownership of public `forward_id` state.
- Preserve explicit target routing and per-target isolation.
- Preserve the current trust model where selecting a side grants network access
  from that machine.
- Preserve the current tunnel transport choice and the resumable listen-side
  reconnect behavior already introduced by the v3 protocol.
- Avoid a new compatibility branch for older daemons. This remains a
  coordinated broker-daemon contract.
- Keep the current Linux/Windows and XP split instead of forcing a broad shared
  platform abstraction.

## Non-Goals

- No public MCP schema redesign for `forward_ports`.
- No replacement of the existing HTTP Upgrade tunnel with WebSocket, HTTP/2, or
  daemon-to-daemon transport.
- No preservation of active TCP streams across reconnect.
- No preservation of UDP per-peer connector state across reconnect.
- No direct broker bypass or peer-to-peer forwarding path between daemons.
- No broad rewrite of unrelated broker tools.
- No attempt in this change to unify Rust and C++ implementations into shared
  generated protocol bindings.

## References

- `crates/remote-exec-broker/src/port_forward.rs`
- `crates/remote-exec-broker/src/tools/port_forward.rs`
- `crates/remote-exec-broker/src/tools/targets.rs`
- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-broker/src/state.rs`
- `crates/remote-exec-host/src/port_forward.rs`
- `crates/remote-exec-daemon/src/port_forward.rs`
- `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`
- `crates/remote-exec-proto/src/port_tunnel.rs`
- `tests/e2e/multi_target.rs`
- `docs/superpowers/specs/2026-05-06-port-forward-upgrade-tunnel-design.md`
- `docs/superpowers/specs/2026-05-06-port-forward-reconnect-design.md`
- `docs/superpowers/specs/2026-05-04-host-runtime-boundary-and-error-classification-design.md`

## Problem Statement

The current implementation works and has solid end-to-end coverage, but the
internal ownership model is split across the wrong boundary.

Today:

- the broker owns public forward state and also owns a large amount of
  transport-specific stream/session orchestration
- the host runtime owns retained listeners, resume windows, and actual local
  socket resource lifetimes
- the broker decides reconnect and failure semantics based partly on ad hoc
  transport classification and string-shaped errors
- the broker exposes only a boolean public target capability even though remote
  forwarding actually depends on a protocol version contract
- the TCP forwarding path can accumulate unbounded pending data before the
  connect side finishes opening

This creates several design problems:

1. public forward state does not map cleanly to underlying resource lifetime
2. failure semantics differ depending on which side happens to be the listen
   side
3. typed tunnel metadata is reduced to strings before policy decisions are made
4. the broker module has grown into a mixed transport, protocol, orchestration,
   and policy file
5. Rust and C++ daemons must match behavior that is partly defined in broker
   implementation details rather than a small explicit control contract

The issue is not that the port-forward subsystem lacks features. The issue is
that the ownership boundary is ambiguous, which makes teardown, reconnect,
error handling, and future edits harder than they should be.

## Current Design Problems

### 1. Public failure and resource teardown are not aligned

The explicit close path and the failure path follow different rules.

On intentional close:

- the broker cancels the runtime
- the broker tries to close the listen-side session explicitly
- the host runtime clears the retained listener or UDP bind immediately

On failure:

- the broker cancels the runtime and marks the public entry as `failed`
- the listen-side session can remain detached until the reconnect timeout
  expires
- the effective cleanup moment is driven by reconnect expiry rather than by the
  public forward state transition

That asymmetry is necessary for retryable transport loss, but the current code
does not model the difference explicitly enough. The result is that a forward
may be publicly failed while the daemon still retains the listener for a grace
period.

### 2. The broker has too much knowledge of tunnel internals

The broker currently owns all of the following in one module:

- tunnel construction for remote and local sides
- session open and resume control flow
- TCP stream pairing
- UDP peer-to-connector management
- reconnect retry policy
- transport failure classification
- tunnel error parsing
- public store updates

Those responsibilities belong to at least three conceptual layers:

- public MCP-facing forward management
- bridge/orchestration between two forward-side sessions
- tunnel/frame transport

Because those layers are collapsed together, small behavioral changes require
editing a broad file and reasoning about too many concerns at once.

### 3. Error metadata is structured on the daemon side but effectively
### stringly-typed on the broker side

The tunnel protocol already carries structured error metadata with fields such
as `code`, `message`, and `fatal`.

However, the broker currently converts that structured metadata into free-form
`anyhow` text and then applies reconnect policy partly by inspecting error
strings or generic transport failures. That is brittle:

- C++ and Rust implementations must preserve message wording accidentally
- reconnect policy cannot depend on a stable internal enum
- the `fatal` concept is present in the protocol but not meaningfully consumed
  at the policy layer

This repeats the same class of problem the earlier host-runtime boundary and
error-classification refactors were intended to eliminate in other features.

### 4. Backpressure is incomplete in the TCP open path

For TCP forwards, the broker may receive listen-side data before the connect
side has answered `TCP_CONNECT_OK`.

The current design queues those frames in memory per pending stream. There is
no explicit byte cap or queue cap. A slow, blackholed, or overloaded
destination can therefore turn the broker into an unbounded memory buffer.

That is not a local bug. It is a missing state-policy rule in the design.

### 5. Public capability reporting is under-specified

`list_targets` currently exposes `supports_port_forward` but not the internal
`port_forward_protocol_version` that the broker actually requires for remote
forwarding.

This means the broker can report a target as port-forward capable while still
rejecting it at open time because the protocol version is too old. The public
target report is therefore not truthful enough for operators.

### 6. Reconnect semantics are encoded as side-specific broker behavior

The reconnect design intentionally preserves listen-side state and recreates the
connect side statelessly. That asymmetry is acceptable.

What is not ideal is that the broker currently enforces that policy in an
implicit way by treating listen-side transport failures as special and
connect-side failures as terminal inside the event loop. The design should make
that policy explicit as part of a smaller bridge model rather than letting it
emerge from large runtime loops.

## Design Summary

Adopt a two-part refactor:

1. move port-forward side-session ownership semantics decisively into the host
   runtime and daemon implementations
2. reduce the broker to a thinner public control plane plus a narrow
   bridge/orchestrator layer that works on typed events instead of raw tunnel
   details

This is not a new transport design. The existing upgrade tunnel and resumable
listen-side session model remain.

This is also not a daemon-only change. The broker still owns public
`forward_id`, public list/close operations, and pairing of listen-side and
connect-side activity. But the broker should stop owning the fine-grained
session lifecycle rules that determine what a detached forward-side session
means on the machine doing the networking.

## Chosen Approach

Use the current tunnel protocol and capability versioning, but reorganize the
implementation around four explicit layers.

### Layer 1: Public broker forward management

Responsibilities:

- validate user input for `forward_ports`
- resolve `listen_side` and `connect_side`
- allocate and store public `forward_id`
- expose `open`, `list`, and `close`
- expose public `status` and `last_error`

This layer must not know raw tunnel frame details.

### Layer 2: Broker bridge/orchestration

Responsibilities:

- open a forward-side session on each side
- consume typed events from the listen side
- open matching connect-side resources
- relay TCP/UDP payloads between sides
- apply explicit reconnect policy
- report terminal failure vs clean close to the public store

This layer should depend on a narrow typed internal interface such as:

```text
ForwardSideSession
  recv_event() -> ForwardSideEvent
  tcp_listen(endpoint)
  udp_bind(endpoint)
  tcp_connect(stream_id, endpoint)
  send_tcp_data(stream_id, bytes)
  send_tcp_eof(stream_id)
  send_udp_datagram(stream_id, peer, bytes)
  close_stream(stream_id)
  close_session(mode)
```

The exact Rust surface can differ, but the design goal is fixed: bridge logic
works with typed session operations, not frame JSON and transport heuristics.

### Layer 3: Forward-side session runtime

Responsibilities:

- own the local interpretation of session open, resume, detach, and close
- retain only the resources that should survive reconnect
- destroy resources immediately for terminal close/failure modes
- classify errors into stable internal categories

This layer belongs in `remote-exec-host` so the broker-local and Rust daemon
paths continue sharing the same state machine.

### Layer 4: Tunnel transport and codec

Responsibilities:

- upgrade HTTP connection where applicable
- validate tunnel preface and frame structure
- read/write framed messages
- map codec/transport problems to typed session-level errors

This layer should remain small and transport-oriented.

## Internal Ownership Model

The refactor makes the session lifetime model explicit.

### Public forward lifetime

Owned by the broker.

State:

- `Open`
- `Closing`
- `Closed`
- `Failed`

This state drives the public MCP result and list output.

### Forward-side session lifetime

Owned by the host runtime or daemon implementation for one side of the forward.

State:

- `Attached`
- `DetachedForResume`
- `Closed`

Resources that may survive detach:

- the listen-side TCP listener
- the listen-side UDP bind
- the accepted-stream ID allocator
- reconnect deadline metadata

Resources that must not survive detach:

- accepted TCP streams
- connect-side outbound TCP streams
- per-connection writer maps
- connect-side UDP per-peer connectors
- any old transport writer task

### Bridge lifetime

Owned by the broker runtime task for one public `forward_id`.

State:

- `Running`
- `RecoveringListenSide`
- `Stopping`
- `TerminalFailure`

The bridge owns only pairing and relay state. It does not own local listener
retention policy.

## Session Close Modes

The existing design effectively has multiple meanings for “the tunnel went
away.” This refactor makes them explicit.

Every forward-side session close must be classified as one of:

- `GracefulClose`
  - intentional user close or broker shutdown
  - teardown retained resources immediately
- `RetryableDetach`
  - transient transport loss on a resumable listen-side session
  - retain resumable resources until deadline
- `TerminalFailure`
  - unrecoverable protocol error, explicit terminal daemon error, or exhausted
    reconnect policy
  - teardown retained resources immediately

This classification replaces the current design where those outcomes are partly
encoded by call site and partly inferred from transport behavior.

## Broker Architecture

The broker should stop treating port forwarding as a single monolith.

### Target module split

Refactor `crates/remote-exec-broker/src/port_forward.rs` into a module tree
with responsibilities like:

- `port_forward/mod.rs`
  - public exports and top-level wiring
- `port_forward/store.rs`
  - `PortForwardStore`, filters, entry updates
- `port_forward/side.rs`
  - `SideHandle`, target-vs-local side setup
- `port_forward/session.rs`
  - `ForwardSideSession` trait or focused adapter type
- `port_forward/events.rs`
  - typed broker-side event and error enums
- `port_forward/tcp_bridge.rs`
  - TCP accept/connect pairing and relay logic
- `port_forward/udp_bridge.rs`
  - UDP peer mapping and connector lifecycle
- `port_forward/supervisor.rs`
  - open/close/fail transitions and reconnect policy
- `port_forward/tunnel.rs`
  - framed transport wrapper and codec glue

The exact filenames may shift, but the split by responsibility should remain.

### Broker-facing internal types

Introduce typed internal shapes for clarity:

```text
enum ForwardSideEvent {
  ListenerReady { stream_id, bound_endpoint },
  TcpAccepted { listener_stream_id, stream_id, peer },
  TcpConnected { stream_id },
  TcpData { stream_id, data },
  TcpEof { stream_id },
  UdpDatagram { stream_id, peer, data },
  StreamClosed { stream_id },
  RetryableTransportLoss,
  TerminalError(ForwardSideError),
}

enum ForwardSideError {
  CapabilityMismatch,
  InvalidProtocol,
  InvalidMetadata,
  BindFailed,
  ConnectFailed,
  ReadFailed,
  WriteFailed,
  UnknownStream,
  ResumeExpired,
  SessionAlreadyAttached,
  SessionUnknown,
  TransportClosed,
}
```

The bridge layer should use these typed values for policy decisions.

### Public state transitions

Rules:

- `open` remains public while retryable listen-side recovery is in progress
- `last_error` remains unset during a successful recovery attempt
- `failed` is set only when recovery is exhausted or the failure is terminal
- terminal failure immediately triggers session close with `TerminalFailure`
  semantics

The current behavior where the public state can become `failed` while retained
resources still wait for timeout must be eliminated.

## Host Runtime Architecture

`remote-exec-host` should become the clear owner of forward-side session
semantics for both broker-local and Rust daemon paths.

### Recommended module split

Refactor `crates/remote-exec-host/src/port_forward.rs` into:

- `port_forward/mod.rs`
- `port_forward/session_store.rs`
- `port_forward/session.rs`
- `port_forward/tcp.rs`
- `port_forward/udp.rs`
- `port_forward/tunnel.rs`
- `port_forward/codec.rs`
- `port_forward/error.rs`

### Runtime responsibilities

The host runtime should own:

- creation and storage of session IDs
- attachment and detachment of live transports
- cleanup policy for close vs detach vs terminal failure
- retained listener and retained UDP bind resources
- per-session accepted-stream allocation
- mapping local socket problems into typed internal errors

### Session API design

The runtime should expose explicit operations along these lines:

```text
open_session() -> SessionOpenResult
resume_session(session_id) -> SessionResumeResult
close_session(session_id, CloseMode)
open_listener(session_id, protocol, endpoint) -> bound_endpoint
open_connector(session_id, stream_id, endpoint)
close_stream(session_id, stream_id)
```

The tunnel implementation can still call lower-level helpers, but the runtime
boundary should express intent directly.

### Detach semantics

On retryable listen-side transport loss:

- move session to `DetachedForResume`
- close all non-resumable resources immediately
- keep resumable listener or bind alive
- start the resume deadline

On terminal failure:

- remove the session from the store immediately
- release retained resources immediately
- cancel all remaining loops immediately

This is the key behavioral correction in the design.

## Tunnel Protocol Handling

The frame protocol itself remains the one already defined by the upgrade tunnel
and reconnect designs. This refactor changes how it is consumed internally.

### Protocol status

Keep:

- `POST /v1/port/tunnel`
- the current framed binary tunnel transport
- `SESSION_OPEN`, `SESSION_READY`, `SESSION_RESUME`, `SESSION_RESUMED`
- the current capability requirement of
  `supports_port_forward && port_forward_protocol_version >= 3`

Do not introduce a new public port-forward RPC family.

### Error handling changes

The tunnel reader/writer layer should decode structured error frames into
typed internal errors instead of collapsing them into free-form strings.

Required rules:

- transport/codec failures become transport-class session errors
- daemon error frames map by stable `code`
- reconnect policy uses typed classification, not substring matching
- the `fatal` field, if retained, must affect classification explicitly rather
  than being ignored

If `fatal` is not precise enough, replace it internally with a richer typed
classification before the broker policy layer sees it.

## Backpressure And Buffering Rules

The refactor must define explicit TCP pending-data limits.

### Required behavior

Before `TcpConnectOk`, the broker must not buffer unlimited data for a pending
paired stream.

Acceptable strategies:

1. stop reading from the listen-side socket until the connect side is ready
2. cap pending bytes and fail the stream when the cap is exceeded

The implementation may choose either strategy, but it must satisfy these rules:

- bounded memory per pending TCP stream
- bounded aggregate memory per forward
- deterministic behavior when the bound is hit
- a stable error category for the overflow/pressure case

The design does not require buffering to disappear entirely. It requires
buffering to become policy-driven and bounded.

### UDP state bounds

UDP connector bookkeeping should also have explicit bounds:

- maximum idle connector lifetime
- maximum concurrent connector count per public forward
- deterministic cleanup when limits are exceeded

The current idle timeout behavior may remain, but it should live in a focused
UDP bridge module rather than a large mixed loop.

## Capability Reporting

The public `list_targets` result should expose enough information to tell the
truth about remote forwarding support.

### Change

Extend `ListTargetDaemonInfo` with:

```rust
pub port_forward_protocol_version: Option<u32>,
```

Rules:

- remote targets report the cached daemon protocol version when known
- `local` handling remains truthful for broker-local behavior
- text formatting for `list_targets` should include the protocol version when
  port forwarding is supported

This is a public schema addition, but it is additive and aligns the target
report with behavior the broker already enforces internally.

## C++ Daemon Architecture

The C++ daemon should mirror the same ownership model even though its runtime
style remains thread-oriented and XP-compatible.

### Recommended split

Break `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp` into focused units
with responsibilities like:

- `port_tunnel_session.cpp`
  - session store, attach, detach, resume, expiry
- `port_tunnel_transport.cpp`
  - frame read/write and upgrade transport lifecycle
- `port_tunnel_tcp.cpp`
  - listener creation, accept loops, TCP stream handling
- `port_tunnel_udp.cpp`
  - UDP bind handling and datagram loops
- `port_tunnel_error.cpp`
  - internal error mapping and response helpers

The exact split can vary, but the C++ daemon should no longer mix:

- session registry logic
- stream/socket lifecycle
- frame parsing
- error response shaping

in one large file.

### Behavioral parity requirements

The C++ daemon must implement the same close-mode semantics as the Rust host
runtime:

- graceful close tears down immediately
- retryable detach retains only resumable listen-side resources
- terminal failure tears down immediately

The broker should not need language-specific policy branches.

## Testing Strategy

This refactor needs focused tests at each layer plus end-to-end coverage.

### Broker-focused tests

Add or update tests for:

- typed tunnel error classification
- public state transitions during reconnect
- immediate teardown on terminal failure
- bounded TCP pending buffering behavior
- UDP connector bound/cleanup behavior
- `list_targets` capability reporting including protocol version

### Host runtime tests

Add focused tests for:

- detach vs graceful close vs terminal failure
- session removal timing
- retained listener survival only for retryable detach
- immediate retained-resource teardown on terminal failure
- typed error mapping from socket/runtime failures

### Rust daemon tests

Add or update tests for:

- tunnel error frame decoding and classification
- close-mode propagation from transport loss vs explicit close
- immediate cleanup on terminal session close

### C++ daemon tests

Add or update tests for:

- session detach/expiry behavior
- immediate terminal teardown semantics
- parity of resume-expired and unknown-session errors
- transport error mapping without broker string matching assumptions

### End-to-end tests

Keep and extend `tests/e2e/multi_target.rs` for:

- reconnect after live listen-tunnel drop
- failure after daemon restart
- release of remote listeners after graceful broker stop
- release of remote listeners after broker crash
- explicit proof that a terminal failure does not leave a stale listener
  accepting until timeout
- bounded behavior when connect-side open is delayed or fails under load

The last item is important because the current suite validates cleanup and
reconnect but does not yet pin down the buffering policy.

## Docs And Public Contract Updates

Update all relevant docs together if the spec is implemented:

- `README.md`
  - clarify capability reporting and any user-visible target info changes
- `skills/using-remote-exec-mcp/SKILL.md`
  - reflect the more precise capability reporting if the skill discusses
    `list_targets`
- any config example comments only if configuration changes are introduced

This refactor should otherwise preserve the documented trust model and the
public `forward_ports` user workflow.

## Sequencing

Implement in stages.

### Stage 1: Typed broker-side protocol boundary

- introduce typed tunnel error decoding and event types
- stop making reconnect decisions from message text
- keep current behavior otherwise

### Stage 2: Broker module split

- split store, side setup, tunnel transport, TCP bridge, UDP bridge, and
  supervisor code into focused modules
- preserve behavior while shrinking responsibility per file

### Stage 3: Host runtime session close-mode refactor

- introduce explicit close-mode semantics
- ensure terminal failure tears down retained resources immediately
- preserve retryable detach behavior for listen-side reconnect

### Stage 4: Bounded buffering and UDP state policy

- add bounded TCP pre-connect buffering or read suppression
- add explicit UDP connector limits

### Stage 5: Capability reporting cleanup

- expose protocol version through `list_targets`
- update docs and tests accordingly

### Stage 6: C++ parity

- apply the same session and error model to the C++ daemon
- keep broker policy shared across Rust and C++

This sequence is intentionally incremental so the safety-critical teardown and
policy fixes can land without requiring a single giant rewrite.

## Risks

### 1. Regressing reconnect behavior while improving teardown semantics

Mitigation:

- keep retryable detach as a first-class close mode
- preserve existing reconnect e2e tests while adding stricter terminal cleanup
  tests

### 2. Adding a public capability field without updating all target-report paths

Mitigation:

- update public proto, broker formatter, tests, and any skill/doc references in
  one coordinated change

### 3. Over-refactoring files before behavior is pinned down

Mitigation:

- land typed event/error seams first
- keep staged behavior-preserving module splits before larger policy changes

### 4. Divergence between Rust and C++ daemon semantics

Mitigation:

- define close-mode behavior once in tests and mirror it in both
  implementations
- prefer broker policy that depends on typed protocol categories instead of
  implementation wording

## Acceptance Criteria

The refactor is complete when all of the following are true:

- the public `forward_ports` API remains stable
- the broker no longer decides reconnect policy from string-matched error text
- terminal forward failure tears down retained listen-side resources
  immediately
- retryable listen-side transport loss still preserves future accepts and
  future UDP datagrams within the reconnect window
- TCP pending buffering is explicitly bounded by policy
- `list_targets` reports enough information to tell whether a target meets the
  remote port-forward protocol requirement
- broker port-forward code is split into focused modules with narrower
  responsibilities
- Rust and C++ daemon implementations both follow the same explicit session
  close model
