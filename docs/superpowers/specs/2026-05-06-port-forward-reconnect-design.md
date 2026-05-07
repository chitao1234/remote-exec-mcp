# Port Forward Reconnect Design

## Goal

Allow an existing public `forward_ports` forward to survive a transient
broker-daemon transport disconnect when the daemon process stays alive.

Recovery preserves:

- the public forward itself
- the remote listen socket or UDP bind
- future TCP accepts and future UDP datagrams after reconnection

Recovery does not preserve:

- accepted TCP streams that were already active when the disconnect happened
- connect-side outbound TCP streams that were already active
- UDP per-peer connector state that existed before the disconnect
- any state across daemon restart
- any state across broker process restart

The public MCP schema for `forward_ports` remains unchanged.

## Current Behavior

The current tunnel protocol intentionally makes the upgraded broker-daemon
socket the ownership boundary for all port-forward resources.

Today:

- the broker opens `/v1/port/tunnel` and speaks the binary tunnel protocol
- the daemon binds listeners or opens sockets on behalf of that tunnel
- if the upgraded transport closes, the daemon immediately destroys every
  listener, UDP socket, and TCP stream owned by that tunnel
- the broker treats any tunnel read failure as terminal and marks the public
  `forward_id` as `failed`

This behavior is implemented in:

- [crates/remote-exec-broker/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward.rs)
- [crates/remote-exec-host/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward.rs)
- [crates/remote-exec-daemon/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon/src/port_forward.rs)
- [crates/remote-exec-daemon-cpp/src/port_tunnel.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel.cpp)

That model is simple and correct for cleanup, but it makes a brief transport
drop indistinguishable from a terminal failure.

## Scope and Non-Goals

This design covers only transient transport failures between a live broker and a
live daemon.

Non-goals:

- recovering after daemon restart
- recovering after broker restart or crash
- preserving active TCP streams through reconnect
- preserving UDP connector streams or in-flight datagrams through reconnect
- adding a new public `forward_ports` action or result field
- restoring the old lease-renewal model
- introducing direct daemon-to-daemon forwarding

Because the daemon cannot distinguish a broker crash from a transient network
drop at the moment the socket disappears, unexpected broker disappearance will
also keep resumable listener state alive for a short grace window before final
cleanup. Graceful broker-driven `close` remains immediate.

## Chosen Approach

Introduce resumable listen-side tunnel sessions with a bounded reconnect
window.

The key design choice is to retain only the state required to keep the public
forward alive:

- the listen-side TCP listener or UDP bound socket survives
- the connect-side tunnel does not survive and is recreated as needed
- active per-connection stream state is discarded on disconnect

This is intentionally asymmetric.

For a TCP forward, the remote listen side is the only side that must retain
state to preserve the forward. The connect side only needs a working tunnel when
a new accepted connection is being paired.

For a UDP forward, the listen-side bind must survive. The per-peer connector
streams on the connect side are explicitly non-resumable and are rebuilt on
demand after reconnect.

This keeps retained daemon state small and avoids impossible guarantees about
in-flight traffic.

## Capability and Versioning

The reconnect feature changes both the internal capability contract and the
tunnel wire protocol.

Recommended versioning:

- bump `TargetInfoResponse.port_forward_protocol_version` from `2` to `3`
- bump the tunnel header `X-Remote-Exec-Port-Tunnel-Version` from `1` to `2`

Meaning:

- `port_forward_protocol_version = 2`: tunnel-based forwarding without resume
- `port_forward_protocol_version = 3`: tunnel-based forwarding with listen-side
  reconnect support
- tunnel header version `2`: wire protocol supports session control frames used
  for resume

Recommended compatibility rule:

- local forwarding continues to work with the in-process tunnel path
- remote forwarding in this coordinated change requires
  `port_forward_protocol_version >= 3`
- the broker does not implement a mixed fallback path that keeps v2 daemons
  working without reconnect support

This matches the repository's existing preference for coordinated broker-daemon
contract upgrades over long-lived compatibility branches.

## Protocol Changes

Keep the existing `POST /v1/port/tunnel` upgrade endpoint. Extend the frame
protocol with tunnel-level control frames on `stream_id = 0`.

New control frame types:

- `SESSION_OPEN`
- `SESSION_READY`
- `SESSION_RESUME`
- `SESSION_RESUMED`

Suggested semantics:

1. Broker opens a new remote listen tunnel and writes the existing preface.
2. Broker sends `SESSION_OPEN`.
3. Daemon replies with `SESSION_READY` containing:
   - `session_id`
   - `resume_timeout_ms`
4. Broker sends `TCP_LISTEN` or `UDP_BIND` as today.
5. If the transport drops unexpectedly, the daemon marks the session detached,
   closes non-resumable state, and starts the reconnect deadline.
6. Broker reconnects to `/v1/port/tunnel`, writes the preface, and sends
   `SESSION_RESUME { session_id }`.
7. Daemon replies with `SESSION_RESUMED` if the detached session still exists.
8. Broker resumes normal operation without replaying `TCP_LISTEN` or `UDP_BIND`.

Failure responses:

- `unknown_port_tunnel_session`
- `port_tunnel_resume_expired`
- `port_tunnel_already_attached`

The broker treats those failures as terminal for the public forward and marks it
`failed`.

## Session Ownership Model

The current ownership rule is:

- upgraded transport lifetime owns all resources

The new rule is:

- session lifetime owns the listen-side listener or UDP bind
- attached transport lifetime owns all active stream-level state

Retained across disconnect:

- TCP listener socket created by `TCP_LISTEN`
- UDP socket created by `UDP_BIND`
- listener stream ID
- daemon-side next accepted-stream ID allocator
- reconnect deadline metadata

Discarded immediately on disconnect:

- accepted TCP streams
- outbound TCP connect streams
- TCP writer maps
- UDP per-peer connector state
- in-flight frame writer task bound to the old transport

This split is the core mechanism that makes forward-level recovery feasible
without pretending that connection-level recovery exists.

## Broker Architecture

Broker changes live primarily in
[crates/remote-exec-broker/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward.rs)
and
[crates/remote-exec-broker/src/daemon_client.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/daemon_client.rs).

### Listen-side tunnel handling

The broker must stop treating every listen-tunnel read error as terminal.

Instead:

- transport-class failure on a remote listen tunnel triggers reconnect logic
- protocol errors or explicit daemon `ERROR` frames remain terminal
- after successful resume, the broker keeps the existing public `forward_id`
  and existing `listen_endpoint`

The broker stores per-forward reconnect metadata:

- remote listen target name
- listen-side `session_id`
- reconnect deadline or retry budget
- current bound listen endpoint
- protocol type

### Connect-side tunnel handling

The connect side is recreated statelessly.

On disconnect handling:

- close all active paired connect-side streams
- drop TCP stream pairing maps
- drop UDP peer-to-connector maps
- reopen the connect-side tunnel lazily or immediately, depending on the
  forward type

Recommended behavior:

- TCP: reopen the connect-side tunnel immediately after listen-side resume so
  the next accepted connection can be paired without an extra latency spike
- UDP: reopen the connect-side tunnel immediately and recreate connector binds
  on demand for each peer

### Public state model

The public `ForwardPortEntry.status` remains unchanged:

- `open`
- `closed`
- `failed`

During reconnect attempts the public status remains `open`.

The broker adds an internal transient recovery state only inside the runtime
task. `last_error` remains unset unless recovery exhausts and the public forward
is finally marked `failed`.

## Rust Daemon Architecture

Rust daemon changes center on
[crates/remote-exec-host/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward.rs)
plus the upgrade endpoint in
[crates/remote-exec-daemon/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon/src/port_forward.rs).

Add a resumable session registry in the shared host runtime, because the broker
`local` path and the Rust daemon should continue sharing the same forwarding
state machine where possible.

Each session record stores:

- `session_id`
- attachment state: `attached` or `detached`
- reconnect deadline
- retained listener or UDP bind
- next daemon stream ID

Session lifecycle:

1. `SESSION_OPEN` creates a record and binds it to the upgraded transport.
2. `TCP_LISTEN` or `UDP_BIND` populates the resumable resource.
3. Transport loss marks the session detached and starts the expiry deadline.
4. All non-resumable stream state is cancelled and destroyed.
5. `SESSION_RESUME` reattaches a fresh transport if the session has not expired.
6. Expired detached sessions are swept and their retained listener resources are
   closed.

Important constraint:

- only one transport may be attached to a session at a time

## C++ Daemon Architecture

The C++ daemon needs the same externally visible behavior as the Rust daemon.

Changes center on
[crates/remote-exec-daemon-cpp/src/port_tunnel.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel.cpp)
and related headers.

Recommended structure:

- add a session registry keyed by opaque `session_id`
- retain only the listen socket or UDP socket plus reconnect deadline metadata
- close accepted TCP streams and per-peer UDP connector state immediately on
  detach
- allow a new upgraded socket to attach to the detached session
- add a small periodic sweep for expired detached sessions

The implementation must stay within the existing C++11 and Windows
XP-compatible style. This is a targeted lifecycle extension, not a reason to
introduce a separate broad platform abstraction layer.

## TCP Flow After Reconnect

For a TCP public forward with a remote listen side:

1. Broker opens a resumable listen tunnel.
2. Broker opens a normal connect tunnel.
3. Broker sends `TCP_LISTEN`.
4. Daemon replies `TCP_LISTEN_OK`.
5. Normal accepts and data relay proceed as today.
6. If the listen-side transport drops:
   - daemon keeps the listener socket
   - daemon closes accepted TCP streams owned by the old transport
   - broker closes paired connect-side streams and clears pairing maps
7. Broker resumes the listen session over a fresh upgraded connection.
8. Broker reopens the connect-side tunnel if needed.
9. Future accepted connections arrive as fresh `TCP_ACCEPT` frames and are
   paired normally.

Connections that were already established before step 6 are allowed to fail.

## UDP Flow After Reconnect

For a UDP public forward with a remote listen side:

1. Broker opens a resumable listen tunnel.
2. Broker opens a normal connect tunnel.
3. Broker sends `UDP_BIND`.
4. Daemon replies `UDP_BIND_OK`.
5. Normal datagram relay proceeds as today.
6. If the listen-side transport drops:
   - daemon keeps the bound UDP socket
   - broker discards all peer-to-connector mappings
7. Broker resumes the listen session over a fresh upgraded connection.
8. Broker reopens the connect-side tunnel if needed.
9. The next datagram seen from peer `P` creates a fresh connector stream on the
   connect side and relay continues.

No guarantee is made for datagrams that were in flight during the disconnect.

## Retry Policy

The reconnect loop must be bounded and selective.

Retry only on transport-class failures such as:

- unexpected EOF on the upgraded socket
- reqwest or Hyper transport errors while re-opening the tunnel
- TLS or TCP disconnects on the broker-daemon channel after a session already
  existed

Do not retry on:

- invalid preface or invalid frame encoding
- explicit daemon `ERROR` frames for bind, accept, connect, or write failures
- endpoint validation failures
- resume rejection due to unknown or expired session

Recommended policy:

- reconnect budget: 10 seconds total
- backoff: small exponential backoff with jitter
- public status remains `open` while budget remains
- once budget is exhausted, mark the forward `failed`

The first implementation should keep this policy internal and fixed. A new
configuration surface is unnecessary unless operational evidence later demands
it.

## Cleanup Semantics

Cleanup rules become:

- explicit public `close`: immediate daemon cleanup, same as today
- graceful broker shutdown that closes forwards intentionally: immediate cleanup
- unexpected broker disappearance or network drop: keep resumable listener state
  until reconnect deadline expires
- daemon shutdown or restart: all state is lost and resume is impossible

This is an intentional trade-off. Short-lived retained listeners after an
unexpected broker death are the cost of making transient transport disconnects
recoverable.

## Testing

Add or update tests across broker, Rust daemon, C++ daemon, and e2e coverage.

Broker-focused tests:

- listen-tunnel transport drop triggers reconnect instead of immediate `failed`
- successful resume keeps the same public `forward_id`
- resume timeout marks the forward `failed`
- explicit daemon `ERROR` frame remains terminal and is not retried

Rust daemon tests:

- detached resumable TCP listener can be resumed before expiry
- detached resumable UDP bind can be resumed before expiry
- expired detached session releases the listener or socket
- accepted TCP streams are closed on detach

C++ daemon tests:

- same detach, resume, and expiry behavior as the Rust daemon
- recoverable reconnect handling does not destabilize the daemon process

End-to-end tests:

- kill only the port-tunnel transport while keeping the daemon process alive,
  then confirm the listen endpoint stays usable for future connections
- verify active TCP streams fail during the drop and are not promised to
  survive
- verify UDP datagrams after resume still reach the destination
- keep the existing daemon-restart failure test, because restart recovery is
  explicitly out of scope

The existing broker-crash cleanup tests may continue to pass if the reconnect
window stays below their current timeout, but their expectation should be
interpreted as "released within the grace window" rather than "released
immediately."

## Documentation Updates

If implemented, update these together:

- [README.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/README.md)
- [crates/remote-exec-daemon-cpp/README.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/README.md)
- [skills/using-remote-exec-mcp/SKILL.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/skills/using-remote-exec-mcp/SKILL.md)
- target capability/version documentation where `port_forward_protocol_version`
  is described

The docs need to be explicit about the new boundary:

- forwards survive transient broker-daemon transport drops
- existing TCP connections do not
- daemon restart still destroys the forward
- unexpected broker loss may delay cleanup until the resume window expires

## Recommendation

Implement reconnect support as a coordinated protocol upgrade that preserves
only listen-side forward state.

This is the smallest change that satisfies the requirement without reintroducing
lease churn or claiming impossible connection-level continuity. It preserves the
current public API, fits the broker-owned public `forward_id` versus
daemon-private runtime split, and remains compatible with the workspace's Rust
and C++ architecture boundaries.
