# C++ Port Forward Lifecycle Rework Plan

Date: 2026-05-17

Status: planning

## Context

The C++ daemon's v4 port-forwarding implementation has accumulated lifecycle
complexity around retained sessions, TCP listeners, UDP binds, stream workers,
connection attachments, sender shutdown, expiry, and service shutdown. Recent
OpenBSD failures were fixed with targeted changes, but the failures exposed a
larger maintenance problem: resource ownership and lifecycle transitions are
distributed across several objects instead of being represented by a small set
of explicit state machines.

This document proposes an incremental rework that keeps the public protocol and
broker-daemon contract stable while reducing future debug cost.

## Goals

- Preserve the existing v4 tunnel wire protocol and public behavior.
- Keep the C++ daemon C++11-compatible and compatible with the Windows XP build
  target.
- Make resource ownership explicit and mechanically hard to misuse.
- Ensure every budget/counter is released exactly once through RAII ownership.
- Centralize session attach, detach, close, expiry, and retained-resource
  transitions.
- Make thread ownership and shutdown ordering clear.
- Add deterministic lifecycle tests for the races that are currently only
  covered indirectly by stress runs.

## Non-Goals

- Do not redesign the public `forward_ports` API.
- Do not change the v4 tunnel frame format unless a separate protocol task
  requires it.
- Do not introduce broad abstraction layers that only wrap the current implicit
  lifecycle.
- Do not depend on C++14 or newer language features.
- Do not remove Windows XP-compatible build support.

## Current Design Debt

### Split Lifecycle Ownership

The effective lifecycle is currently spread across service, session, connection,
sender, stream, listener, UDP socket, expiry, and worker code. Several different
paths can close sockets, detach sessions, expire sessions, release budgets, wake
threads, and mark resources closed.

This makes correctness depend on call ordering and idempotence rather than on a
single authoritative state transition.

### Manual Budget Accounting

The code tracks resource budgets using manual acquire/release calls and boolean
flags on resource structs. This pattern is fragile when construction partially
succeeds, when a worker fails to start, or when shutdown races with active work.

The intended invariant should be simpler:

- If a budgeted resource exists, it owns a move-only budget lease.
- Destroying or explicitly closing the resource releases the lease exactly once.
- No caller outside the resource needs to remember whether that budget was
  acquired.

### Distributed Close Paths

The implementation has multiple close helpers for streams, UDP sockets,
retained listeners, retained resources, current sessions, connection-local
state, all sessions, and expiry. Most of these helpers are intentionally
idempotent, but the number of valid callers makes it hard to reason about which
thread owns teardown at any given moment.

Idempotence should remain, but it should be a safety property, not the primary
coordination mechanism.

### Ambiguous Thread Ownership

The service tracks some worker threads and budgets, but the sender/writer thread
has separate lifecycle behavior. Expiry and shutdown are another source of
cross-thread coordination. This increases the chance of late callbacks,
use-after-free, missed wakeups, or shutdown hangs.

The service should own the runtime that owns all daemon-side port-forward
threads, or each connection/session should own a clearly bounded worker group
that is joined deterministically.

### Retained Session Complexity

Retained TCP and UDP resources outlive a single broker-daemon HTTP tunnel
connection. That behavior is required for reconnect, but it creates a hard split
between connection lifecycle and session lifecycle.

The retained resources should be session-owned. Connections should attach to and
detach from sessions through a narrow API. Retained workers should publish
events through the current attachment if one exists, rather than directly
participating in connection ownership.

## Target Architecture

### Ownership Model

Use a small set of explicit owners:

- `PortTunnelService` owns the session map, global limits, shutdown state, and
  the port-forward runtime/worker group.
- `PortTunnelSession` owns retained-session state, generation, expiry deadline,
  current attachment, and retained resources.
- `PortTunnelConnection` owns one live HTTP tunnel connection, inbound frame
  decoding, outbound writer, and connection-local resources.
- Resource objects own sockets, close state, wakeup behavior, and budget leases.
- Worker/runtime objects own thread handles and joining behavior.

### Session State Machine

Represent retained session lifecycle through explicit transitions:

```text
New -> Attached -> Detached -> Attached
Attached -> Closing -> Closed
Detached -> Expired -> Closed
Detached -> Closing -> Closed
```

The exact enum names can differ, but the implementation should enforce these
concepts:

- A session can have at most one current attachment.
- Detach records the resume deadline and invalidates connection-owned send
  paths.
- Attach validates generation/session identity before exposing retained
  resources to the new connection.
- Close prevents future attach and starts resource teardown.
- Expiry is a close reason, not a separate teardown mechanism.

### Resource State Machine

Each retained or connection-local resource should have one idempotent close
operation:

```text
Open -> Closing -> Closed
```

The close operation should own:

- socket shutdown/close
- condition-variable wakeups
- budget lease release
- removal from owning maps when appropriate
- final notification behavior when required by the protocol

Resource cleanup may need to return deferred actions so sockets can be closed
outside higher-level locks. The state transition itself should still happen in
one place.

### Budget Leases

Introduce C++11 move-only RAII lease types for all port-forward counters:

- worker budget
- retained session budget
- retained TCP listener budget
- UDP bind budget
- active stream budget

The lease should be non-copyable, movable, and safe to destroy empty. Resource
objects should store the relevant lease directly.

Example shape:

```cpp
class CounterLease {
 public:
  CounterLease();
  CounterLease(PortTunnelService* service, CounterKind kind);
  CounterLease(CounterLease&& other);
  CounterLease& operator=(CounterLease&& other);
  ~CounterLease();

  void reset();
  bool valid() const;

 private:
  CounterLease(const CounterLease&);
  CounterLease& operator=(const CounterLease&);

  PortTunnelService* service_;
  CounterKind kind_;
};
```

Specific typed leases may be preferable to a generic `CounterKind` if that keeps
call sites clearer.

### Runtime And Worker Ownership

Introduce a `PortTunnelRuntime` or `WorkerGroup` that owns:

- thread handles
- worker budget leases
- sender/writer thread handles
- expiry thread handle or expiry task
- shutdown flag propagation
- deterministic join/drain behavior

Rules:

- No tracked worker should outlive the runtime that owns it.
- No sender thread should outlive its connection.
- Service shutdown should close sessions first, wake all blocking resources,
  then join worker groups.
- A worker must not need to join itself; use a no-join-from-self rule or a
  cleanup queue.

### Connection And Sender Model

Each connection should have one outbound writer lifecycle:

- Workers enqueue outbound frames through a thread-safe queue.
- The connection writer drains that queue.
- Closing the connection closes the queue.
- Sends after close fail deterministically.
- The writer is joined before the connection is considered fully closed.

This narrows concurrency. Worker threads no longer need to coordinate directly
with socket write shutdown; they only need to handle queue rejection.

## Phased Implementation Plan

### Phase 1: Add RAII Budget Leases

- Introduce move-only leases for worker and resource budgets.
- Migrate retained session, retained listener, UDP bind, and active stream
  booleans to leases.
- Remove manual release calls from partial construction and failure paths where
  the lease can own cleanup.
- Add tests for partial construction failure and double-close safety.

Expected result: budget accounting becomes local to resource ownership.

### Phase 2: Resource-Owned Close Methods

- Move socket close, wakeup, closed flag, and lease release into resource
  methods.
- Replace free-function close protocols with methods on retained listener, TCP
  stream, and UDP socket objects.
- Keep close idempotent.
- Ensure callers can request close without knowing whether the resource still
  owns a budget.

Expected result: each resource has one authoritative teardown path.

### Phase 3: Centralize Session Transitions

- Add explicit `PortTunnelSession` methods for attach, detach, close, expire,
  install resource, and remove resource.
- Move generation checks, attachment replacement, resume deadline handling, and
  retained resource teardown behind those methods.
- Make expiry call the same close transition used by explicit shutdown.
- Return deferred teardown actions where needed to avoid closing sockets while
  holding session/service locks.

Expected result: retained session lifecycle becomes reviewable as one state
machine.

### Phase 4: Centralize Thread And Sender Ownership

- Add a runtime/worker-group owner for port-forward worker threads.
- Bring sender/writer lifecycle under explicit ownership.
- Replace detached sender semantics with close-and-join semantics where
  possible.
- Define shutdown order as:
  1. stop accepting new sessions/connections
  2. close sessions and resources
  3. close outbound queues
  4. wake blocking workers
  5. join workers and writers

Expected result: service destruction no longer depends on scattered late-thread
behavior.

### Phase 5: Simplify Retained TCP/UDP Workers

- Make retained TCP and UDP loops interact with session attachment through a
  narrow API.
- Avoid workers holding or inferring connection ownership directly.
- Ensure detach/reconnect behavior is represented by session state, not by
  ad hoc weak-pointer checks.
- Review UDP recv paths for the same retained-socket synchronization pattern as
  TCP.

Expected result: reconnect behavior remains supported but is easier to reason
about.

### Phase 6: Deterministic Lifecycle Tests

Add tests with synchronization gates or fault-injection hooks for:

- detach while retained TCP accept is blocked
- detach while retained UDP recv is blocked
- close while outbound frames are queued
- session expiry while retained workers wait for attachment
- connection close racing with stream close
- worker-budget failure after partial resource creation
- generation mismatch during resume while old workers unwind
- service shutdown with retained sessions and active streams

Expected result: the failure modes that are currently flaky become forced,
repeatable tests.

### Phase 7: Cleanup And Contract Verification

- Remove redundant close calls that are no longer needed.
- Remove budget-acquired booleans.
- Remove ad hoc detached-thread cleanup paths.
- Verify protocol-visible behavior remains unchanged.
- Keep Rust daemon and C++ daemon behavior aligned for shared contract areas.

Expected result: the implementation is simpler without changing the external
contract.

## Validation Plan

Run targeted C++ checks first:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp test-host-transfer
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
```

For port-forward contract coverage, also run relevant broker tests:

```sh
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
```

For cross-platform compile risk:

```sh
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

When a phase changes shared public behavior or error normalization, run broader
workspace gates according to the repository testing policy.

## Migration Notes

- Keep each phase independently reviewable and testable.
- Prefer moving ownership before changing behavior.
- Preserve existing protocol error codes unless a specific bug requires a
  contract change.
- Avoid combining mechanical RAII migration with behavior changes in the same
  patch.
- Add lifecycle tests before or alongside the phase that makes the race
  impossible.
- Treat OpenBSD as a high-signal platform for teardown ordering because it has
  already exposed bugs hidden on Linux.

## Success Criteria

- Resource close paths are owned by resource objects and are idempotent.
- Budget release is automatic and exactly-once.
- Retained session attach, detach, close, and expiry are visible in one session
  state machine.
- Service shutdown has a documented and enforced order.
- Sender/writer threads are not detached from their owning connection lifecycle.
- Retained TCP/UDP workers do not reach across ownership boundaries.
- Deterministic tests cover the known lifecycle races.
- Existing C++ daemon and broker-facing port-forward tests continue to pass.
