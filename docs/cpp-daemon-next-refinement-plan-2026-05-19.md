# C++ Daemon Next Refinement Plan

Date: 2026-05-19

Status: planning

Scope: `crates/remote-exec-daemon-cpp/`

Related:

- `docs/cpp-daemon-architecture-redesign-plan-2026-05-19.md`
- `docs/cpp-daemon-architecture-audit-2026-05-19.md`
- `docs/cpp-daemon-reliability-followup-plan-2026-05-18.md`
- `docs/cpp-port-forward-lifecycle-rework-plan.md`

## Purpose

This plan defines the next implementation sequence for simplifying the C++
daemon after the header and source relayout work.

The next work should not be another broad move. The public header hierarchy is
now in place, and the remaining architectural pressure is concentrated in the
port-forward lifecycle code. The highest-value next task is to make retained
session ownership, service ownership, expiry, and terminal teardown easier to
reason about before making more compatibility-sensitive socket changes.

## Ground Rules

- Preserve C++11.
- Preserve the thread-based concurrency model.
- Preserve the no-`openat` design.
- Preserve Windows XP-compatible build paths.
- Preserve v4 port-forward protocol behavior.
- Do not change the public broker-daemon contract in this sequence.
- Keep BSD and other POSIX compatibility risks explicit in review.
- Keep GNU make, BSD make, and NMAKE source inventories aligned.
- Commit after each completed task-sized implementation.
- Do not combine behavior changes with mechanical extraction work.

## Current State

The C++ daemon public headers are now nested under subsystem directories such as
`core/`, `platform/`, `http/`, `runtime/`, `rpc/`, `exec/`, `transfer/`, and
`port_forward/`.

The remaining high-risk complexity is not the include layout. It is the
port-forward lifecycle implementation, especially:

- retained tunnel session state,
- connection attachment state,
- retained TCP listener and UDP bind ownership,
- close ordering,
- expiry scheduling,
- service shutdown,
- worker wakeup behavior,
- reconnect and detach transitions.

The main implementation hotspot is
`crates/remote-exec-daemon-cpp/src/port_forward/port_tunnel_session.cpp`, which
currently mixes session transition logic, terminal teardown helpers, retained
resource closing, service registry operations, shutdown, and expiry scheduling.

## Recommended Sequence

### Task 1: Split Service Implementation From Session State

Goal: separate retained session state transitions from service-level ownership
without changing behavior.

Changes:

- Add `src/port_forward/port_tunnel_service.cpp`.
- Move `PortTunnelService` method implementations out of
  `src/port_forward/port_tunnel_session.cpp`.
- Keep `PortTunnelSession` state transition methods in
  `src/port_forward/port_tunnel_session.cpp`.
- Keep `src/port_forward/port_tunnel_service.h` as the private declaration
  boundary.
- Update `mk/sources.mk` so GNU make, BSD make, and NMAKE builds consume the
  new source file.
- Preserve existing lock ordering and shutdown behavior.

Non-goals:

- Do not change v4 tunnel protocol behavior.
- Do not change public capability reporting.
- Do not change worker/thread ownership.
- Do not refactor socket operations in this task.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming test-host-server-runtime test-host-server-routes test-port-tunnel-frame
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp all-windows-xp
git diff --check
```

Commit:

```text
Split C++ port tunnel service implementation
```

### Task 2: Extract Terminal Teardown Helpers

Goal: make terminal close ordering explicit and easier to audit for BSD/POSIX
socket behavior.

Changes:

- Add a private teardown helper module, likely:
  - `src/port_forward/port_tunnel_session_teardown.h`
  - `src/port_forward/port_tunnel_session_teardown.cpp`
- Move helper logic for:
  - connection-local stream closing,
  - attachment closing,
  - retained TCP listener closing,
  - retained UDP bind closing,
  - terminal teardown finalization.
- Keep resource closing outside locks.
- Keep ownership semantics unchanged:
  - retained session owns retained listener or UDP bind,
  - connection attachment owns connection-local streams,
  - service owns session registry and expiry scheduling.

Compatibility notes:

- BSD and other POSIX systems can differ in how blocked socket operations wake
  after close or shutdown from another thread.
- This task should clarify close ownership and ordering, not introduce new
  socket behavior.
- Any observed behavior difference should be fixed through the platform/socket
  boundary where possible, not with scattered call-site workarounds.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming test-host-server-runtime test-host-server-routes test-port-tunnel-frame
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp all-windows-xp
git diff --check
```

Commit:

```text
Extract C++ port tunnel session teardown helpers
```

### Task 3: Clarify Session State Transitions

Goal: make `PortTunnelSession` read like a small explicit state machine.

Changes:

- Keep the existing states:
  - `New`
  - `Attached`
  - `Detached`
  - `Closed`
  - `Expired`
- Group transition logic into visibly distinct helpers:
  - attach transition,
  - detach transition,
  - close transition,
  - expire transition,
  - resume preparation.
- Keep state-name helpers private.
- Keep comments focused on invariants, lock ownership, and close ordering.
- Avoid a larger abstraction unless it clearly removes complexity after the
  first two tasks.

Compatibility notes:

- The state machine should make reconnect and detach behavior easier to verify
  on BSD systems.
- The task should not change deadlines, reconnect semantics, retained resource
  ownership, or stream ID behavior.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming test-host-server-runtime test-host-server-routes test-port-tunnel-frame
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp all-windows-xp
git diff --check
```

Commit:

```text
Clarify C++ port tunnel session transitions
```

### Task 4: Separate Expiry Flow From Registry Flow

Goal: make retained session lifetime easier to audit independently from the
service map of known sessions.

Changes:

- Keep `PortTunnelService` as the owner.
- Internally separate:
  - session registry operations,
  - expiry scheduler operations,
  - shutdown operations.
- Consider a small private helper only if it removes real complexity.
- Keep the lock-ordering comments in `port_tunnel_service.h` accurate.

Compatibility notes:

- Expiry uses condition-variable and thread wakeup behavior that can expose
  POSIX implementation differences.
- Any change must preserve the current shutdown and worker-join guarantees.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming test-host-server-runtime test-host-server-routes test-port-tunnel-frame
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp all-windows-xp
git diff --check
```

Commit:

```text
Separate C++ port tunnel expiry flow
```

### Task 5: Review Port-Forward Close Compatibility

Goal: after ownership and teardown boundaries are clearer, re-audit
port-forward close paths for BSD and other POSIX compatibility issues.

Review points:

- `close`, `shutdown`, `recv`, `send`, `poll`, and `select` behavior under
  interruption.
- Whether EOF and error paths are handled consistently on BSD and Linux.
- Whether socket close wakes blocked worker threads reliably.
- Whether detach and reconnect paths leak retained resources.
- Whether UDP bind and TCP listener teardown have equivalent lifecycle behavior.
- Whether any assumptions remain Linux-specific.

Expected outcome:

- Prefer small fixes through platform/socket wrappers.
- Avoid ad hoc call-site syscall handling unless no cleaner boundary exists.
- Do not address the deferred IPv4 listener limitation in this pass.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp all-windows-xp
git diff --check
```

Commit if changes are needed:

```text
Harden C++ port tunnel close compatibility
```

## Deferred Work

The following work is still important but should not interrupt the above
sequence unless new test failures point directly at it:

- Transfer authorization and archive materialization cleanup.
- Exec/session lifecycle simplification.
- Remaining syscall wrapper consolidation.
- Port-forward socket primitive cleanup beyond close-path compatibility.
- IPv4 listener limitation.
- `LC_ALL` and `LANG` behavior.

## Next Decision Point

After Task 5, choose the next architecture target based on observed risk.

Expected priority:

1. Transfer materialization and sandbox checks.
2. Exec/session lifecycle simplification.
3. Remaining syscall wrapper consolidation.
4. Port-forward socket primitive cleanup.

If the BSD/POSIX close-path review exposes more port-forward compatibility
issues, continue in port forwarding before moving to transfer.
