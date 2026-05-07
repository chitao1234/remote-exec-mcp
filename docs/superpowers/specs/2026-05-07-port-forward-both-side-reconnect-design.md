# Port Forward Both-Side Reconnect Design

**Date:** 2026-05-07

## Goal

Extend `forward_ports` reconnect handling so a transient broker-daemon tunnel
drop is recoverable when either forwarding side loses its upgraded tunnel,
while preserving the current guarantee level:

- the public forward stays alive when recovery succeeds
- future TCP accepts and future UDP datagrams continue to work after recovery
- active TCP streams are still allowed to fail
- UDP per-peer connector state is still allowed to be lost
- broker restart and daemon restart still remain terminal

The public MCP schema does not change.

## Context

The current reconnect work already introduced resumable listen-side tunnel
sessions and the v3 forwarding protocol. That behavior is implemented across:

- [crates/remote-exec-broker/src/port_forward/supervisor.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/supervisor.rs)
- [crates/remote-exec-broker/src/port_forward/tcp_bridge.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/tcp_bridge.rs)
- [crates/remote-exec-broker/src/port_forward/udp_bridge.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/udp_bridge.rs)
- [crates/remote-exec-host/src/port_forward/tunnel.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward/tunnel.rs)
- [crates/remote-exec-daemon/src/port_forward.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon/src/port_forward.rs)
- [crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp)
- [crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp)

There is already a design for listen-side reconnect in:

- [docs/superpowers/specs/2026-05-06-port-forward-reconnect-design.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/docs/superpowers/specs/2026-05-06-port-forward-reconnect-design.md)

That design explicitly says the connect side is recreated statelessly after
disconnect. The current code only does that when the listen side drops first.

## Problem Statement

Today, the broker runtime has an implicit asymmetry:

- listen-side transport loss is treated as retryable
- connect-side transport loss is treated as terminal

This is visible in the bridge loops:

- TCP connect-side `recv()` failure returns `reading tcp connect tunnel`
- UDP connect-side `recv()` failure returns `reading udp connect tunnel`
- only the listen-side path can return a reconnect control signal

As a result, the forward may survive a listen-side tunnel loss but fail for the
same class of transport failure on the connect side, even though the desired
guarantee level is the same in both cases.

The user requirement is to merge these behaviors at the recovery-policy level,
not to make the underlying daemon state fully symmetric.

## Constraints

- Keep the public `forward_ports` request and response schema unchanged.
- Preserve broker ownership of public `forward_id`.
- Preserve explicit side routing and per-target isolation.
- Preserve the existing reconnect guarantee level rather than strengthening it.
- Do not promise active TCP stream recovery.
- Do not promise UDP per-peer connector preservation.
- Do not add a new compatibility branch for older forwarding protocol versions.
- Keep the existing listen-side resumable session protocol.
- Keep the C++ daemon aligned with the same externally visible contract.

## Non-Goals

- No public tool schema change.
- No daemon restart recovery.
- No broker restart recovery.
- No full symmetric session model where the connect side retains daemon-local
  forwarding state for resume.
- No direct daemon-to-daemon forwarding path.
- No new operator configuration surface for reconnect budgets in this change.

## Chosen Approach

Unify reconnect handling at the broker bridge layer, while preserving the
existing daemon-side state asymmetry.

This means:

- both tunnel roles are recoverable from retryable transport loss
- recovery policy is shared
- recovery action depends on the failed role

Role-specific recovery actions:

- listen side:
  - resume the existing tunnel session
  - keep the retained listener or UDP bind alive
  - then reopen the connect-side tunnel because its live stream state is not
    preserved across the generation break
- connect side:
  - reopen a fresh tunnel only
  - drop ephemeral bridge state
  - keep the existing listen-side session and retained resource untouched

This is a policy merge, not a retained-state merge.

## Why Not Make Both Sides Session-Resumable

The current daemon model is deliberately asymmetric:

- the listen side is the only side that must retain a bound TCP listener or UDP
  socket to preserve the public forward
- the connect side only needs a working tunnel when a new outbound TCP connect
  or UDP connector bind is needed

Promoting the connect side into a resumable retained-resource session would add
protocol and daemon complexity without improving the public guarantee:

- active outbound TCP connects still cannot be resumed meaningfully
- UDP per-peer connector streams are already defined as expendable
- reconnect semantics would look more symmetric internally, but the user-visible
  promise would remain the same

The correct merge point is therefore the broker recovery state machine.

## Broker Design

### Recovery Model

Replace the current listen-only reconnect control with a role-aware recovery
signal.

Conceptually:

- `TunnelRole::Listen`
- `TunnelRole::Connect`

And loop control that can request recovery of one role rather than encoding the
policy as a listen-only special case.

The retryability rule remains transport-only:

- unexpected EOF
- connection reset
- broken pipe
- aborted or not-connected transport
- tunnel open failures caused by broker-daemon transport loss

Explicit tunnel `Error` frames remain terminal.

### Shared Recovery Semantics

When either side hits retryable transport loss:

1. keep public forward status as `open`
2. drop all ephemeral runtime state tied to the current tunnel generation
3. recover the failed side according to its role
4. resume accepting future listen-side events
5. mark the forward `failed` only if recovery exhausts or a terminal protocol
   error occurs

Ephemeral state to drop on generation change:

- TCP stream pairing maps
- TCP pending buffered frames awaiting `TcpConnectOk`
- UDP peer-to-connector maps
- any in-flight connect-side tunnel state

This explicitly preserves the existing guarantee level.

### TCP Behavior

For TCP:

- if the listen tunnel fails, use the existing listen session resume logic,
  then reopen the connect tunnel
- if the connect tunnel fails, reopen only the connect tunnel
- in both cases, discard all current pairings and pending frames
- future `TcpAccept` frames create fresh pairings normally

Already-active TCP connections may fail during the transition and are not
recovered.

### UDP Behavior

For UDP:

- if the listen tunnel fails, resume the listen session and reopen the connect
  tunnel
- if the connect tunnel fails, reopen only the connect tunnel
- in both cases, discard all peer-to-connector mappings
- future inbound datagrams recreate connector binds lazily

UDP datagrams in flight during the drop may be lost.

### Retry Budget

Use one reconnect policy regardless of which side failed:

- same retryable transport classification
- same bounded retry window
- same backoff behavior

The current listen-side reconnect timeout logic can remain the source of truth
for the actual retry window. Connect-side reconnect should use the same bounded
policy rather than introducing a separate independent budget.

## Rust Daemon and Shared Host Runtime Design

No new wire protocol is required for Rust daemon or broker-host `local`
forwarding.

The existing host runtime already provides the right asymmetric lifecycle:

- resumable session state for retained listen-side resources
- retryable detach semantics on transport loss
- immediate teardown of non-resumable stream state

That model should stay intact.

Required Rust-side work is therefore limited to:

- validating that no broker assumptions rely on connect-side behavior that the
  host runtime does not provide
- updating comments or tests where they still describe reconnect as
  listen-side-only

No new frame types or RPC schema changes are needed.

## C++ Daemon Design

The C++ daemon should remain behaviorally aligned with the Rust daemon.

It already has the same essential listen-side resume model:

- `SessionOpen`
- `SessionResume`
- retained TCP listeners and UDP binds
- expiry and detach handling

The C++ changes in this design are not to invent a connect-side retained
session. They are to make the implementation and tests explicitly support the
same broker contract:

- transport loss on the connect side is expected to be recoverable by opening a
  fresh tunnel
- retained session behavior remains listen-side-only
- active TCP streams and per-peer UDP connector state remain disposable

Concrete C++ change areas:

- [crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp)
  - keep session detach/close semantics stable and documented relative to the
    broker’s both-side recovery policy
- [crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp)
  - keep retained listener/bind expiry behavior aligned with reconnect wording
- [crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/src/port_tunnel_error.cpp)
  - ensure transport-owned state teardown remains compatible with broker
    generation resets
- [crates/remote-exec-daemon-cpp/README.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/README.md)
  - update wording from listen-side-only reconnect phrasing to both-side
    transport-loss recovery phrasing with unchanged guarantees

If the broker/e2e tests expose any daemon-side mismatch under isolated
connect-side drops, fix that in the C++ implementation as part of this change.

## Testing Design

### Broker and E2E Tests

Add e2e coverage for isolated connect-side tunnel loss, not only whole-forward
tunnel drops.

The simplest way to do this with the existing fixture shape is to reverse the
forward direction for the targeted tests:

- TCP connect-side recovery:
  - `listen_side = local`
  - `connect_side = builder-a`
  - drop builder-a port tunnels
  - confirm the public forward remains `open`
  - verify a new TCP connection still succeeds
- UDP connect-side recovery:
  - `listen_side = local`
  - `connect_side = builder-a`
  - drop builder-a port tunnels
  - confirm a later datagram still relays successfully

Keep the existing tests for remote listen-side reconnect, because they still
exercise the retained-session path.

### C++ Daemon Tests

Extend the existing streaming tests in
[crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp)
to verify the retained-session contract remains correct:

- session resume after transport drop for TCP listener
- expired session releases retained listener

For this change, C++-specific additions should focus on confirming the daemon
still behaves correctly under the broker’s both-side recovery contract, rather
than trying to unit-test broker orchestration in C++.

If needed, add small focused helpers for:

- reconnect wording invariants in test comments or assertions
- retained listener/bind survival across a detached interval

### Documentation

Update together:

- [README.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/README.md)
- [crates/remote-exec-daemon-cpp/README.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/README.md)
- [skills/using-remote-exec-mcp/SKILL.md](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/skills/using-remote-exec-mcp/SKILL.md)

New wording should say:

- transient broker-daemon transport loss on either side may be recovered
- only the forward plus future listen-side traffic are preserved
- active TCP streams and UDP connector state are still lost

## Files Expected to Change

Broker:

- `crates/remote-exec-broker/src/port_forward/events.rs`
- `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- possibly `crates/remote-exec-broker/src/port_forward/supervisor.rs`

Tests:

- `tests/e2e/multi_target.rs`
- possibly `tests/e2e/support/mod.rs` if the current fixture needs a more
  targeted tunnel-drop helper
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`

Docs:

- `README.md`
- `crates/remote-exec-daemon-cpp/README.md`
- `skills/using-remote-exec-mcp/SKILL.md`

## Summary

The correct way to “merge both sides” is:

- one broker recovery policy
- one retryable transport-loss model
- one guarantee level
- two recovery strategies under that shared policy

Listen-side state retention remains special at the daemon level. Connect-side
recovery becomes first-class at the broker level by reopening a fresh tunnel
instead of failing the public forward immediately.
