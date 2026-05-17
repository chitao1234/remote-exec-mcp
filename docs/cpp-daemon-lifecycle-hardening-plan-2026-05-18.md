# C++ Daemon Lifecycle Hardening Plan

Date: 2026-05-18

Status: planning

Related:

- `docs/cpp-port-forward-lifecycle-rework-plan.md`
- Commit `a9f76f0` - `Fix C++ daemon exit drain and tunnel expiry lifecycle`

## Context

Recent C++ daemon flakes exposed two different lifecycle problems:

- DragonFly `test-host-session-store` sometimes missed `"late tail"` under
  `make check-posix -j 8` load because the POSIX session exit drain killed
  descendants after a fixed 125 ms grace window.
- DragonFly `test-host-server-streaming` could abort with `std::terminate()`
  when the port-tunnel expiry scheduler released the final effective service
  owner from inside the scheduler thread, causing the service destructor to try
  to stop or join its own scheduler.

The first bug was a process-output lifecycle policy mismatch. The second bug was
an ownership and shutdown bug. Together they suggest the C++ daemon needs
broader lifecycle hardening, but the highest-risk area is still port forwarding:
it combines retained sessions, reconnect state, TCP listeners, UDP binds,
active streams, worker limits, socket shutdown, expiry, and service shutdown.

This plan is intentionally incremental. The goal is not to rewrite the daemon,
but to make lifecycle ownership explicit enough that future bugs are easier to
debug and harder to introduce.

## Goals

- Preserve the public broker-daemon contract and v4 port-tunnel wire protocol.
- Keep C++11 compatibility and Windows XP-compatible build support.
- Make process/session/output drain behavior explicit and aligned with the Rust
  host where the contract is shared.
- Make port-forward object ownership explicit and acyclic.
- Make shutdown idempotent, ordered, and safe from any participating thread.
- Ensure budget counters are released exactly once without depending on service
  object lifetime.
- Convert known load-sensitive races into deterministic regression tests.
- Commit after each completed task so regressions can be bisected cleanly.

## Non-Goals

- Do not redesign the public `forward_ports` API.
- Do not change the v4 tunnel frame format.
- Do not introduce C++14 or newer requirements.
- Do not merge the Rust and C++ implementations.
- Do not do a broad daemon rewrite that mixes unrelated HTTP, transfer, patch,
  image, and exec behavior changes.

## Working Principles

- Each task must leave the tree buildable and testable.
- Prefer adding a regression test before or alongside the lifecycle change.
- Prefer explicit state transitions over destructor side effects.
- Destructors may perform safe cleanup, but they should not be the primary
  coordinator for complex shutdown.
- Avoid holding service or session locks while closing sockets, joining
  threads, or running callbacks that can release ownership.
- Avoid strong ownership cycles. If a thread must keep an object alive, the
  owning scope and release point must be deliberate and documented in code.
- Budget accounting should not require reviving or retaining
  `PortTunnelService`.
- When changing Windows-shared code, run `check-windows-xp`; run under Wine when
  the make target requires it and Wine is available.

## Known Failure Classes

### Exit Output Drain Races

The daemon needs to handle a command that exits while descendants still have a
stdout or stderr pipe open. A fixed small grace window is fragile under load.

Current mitigation:

- The C++ daemon now uses an idle grace and max grace before terminating
  descendants after parent exit.

Remaining hardening:

- Document the intended output-drain contract in code or tests.
- Add boundary tests for output arriving just before idle grace, output arriving
  repeatedly until max grace, and no output until max grace.
- Compare C++ behavior with Rust host constants when shared behavior changes.

### Self-Join And Destructor Reentry

The expiry scheduler crash showed that destruction can happen on a participating
thread. Self-join handling avoids `std::terminate()`, but the deeper issue is
that meaningful teardown can be triggered by budget/resource destruction while
another lifecycle method is still on the stack.

Current mitigation:

- The expiry scheduler keeps the service alive while it is active.
- The scheduler can stop safely from its own thread.
- The scheduler exits when only the scheduler owns the service and there are no
  scheduled sessions left.

Remaining hardening:

- Remove budget-release dependency on `PortTunnelService` object lifetime.
- Centralize thread ownership and self-thread behavior.
- Make service shutdown a state transition, not a destructor cascade.

### Retained Resource Teardown Races

Retained TCP listeners and UDP binds outlive a single HTTP tunnel connection.
That is required for reconnect, but it splits resource lifetime from connection
lifetime. Bugs appear when detach, expiry, close, and reconnect race.

Remaining hardening:

- Make `PortTunnelSession` the single owner of retained resources.
- Make each retained resource own its socket, wakeups, close state, and budget
  lease.
- Return deferred teardown actions where needed so callers can release locks
  before closing sockets.

### Worker And Sender Lifetime Races

Worker threads, sender threads, accept loops, read loops, write loops, and the
expiry scheduler use related but not identical ownership patterns.

Remaining hardening:

- Use one thread-owner abstraction or one consistent pattern for all
  port-forward threads.
- Define shutdown ordering for connection writers, retained workers, and service
  workers.
- Ensure no thread can outlive the runtime state it touches.

## Target Lifecycle Model

### Service

`PortTunnelService` owns:

- global lifecycle state: `Running`, `Stopping`, `Stopped`
- session map
- port-forward limits and budget state
- expiry scheduler
- worker/runtime owner

Service shutdown order should be:

1. Transition from `Running` to `Stopping`.
2. Stop accepting new sessions and new retained resources.
3. Remove sessions from the service map.
4. Transition sessions to terminal close states.
5. Close retained resources and connection-local resources outside service
   locks.
6. Close outbound queues and wake blocking workers.
7. Join worker, writer, and scheduler threads.
8. Transition to `Stopped`.

### Session

`PortTunnelSession` owns:

- session identity and generation
- attachment state
- resume deadline
- retained-session budget lease
- retained TCP listener or UDP bind
- terminal close reason

Target state machine:

```text
New -> Attached
Attached -> Detached -> Attached
Attached -> Closing -> Closed
Detached -> Expiring -> Closing -> Closed
Detached -> Closing -> Closed
```

Rules:

- A session has at most one attachment.
- Attach validates generation before exposing retained resources.
- Detach records a resume deadline and invalidates connection-owned send paths.
- Expiry uses the same terminal teardown path as explicit close.
- Session methods return teardown work rather than closing sockets under locks.

### Connection

`PortTunnelConnection` owns:

- one upgraded HTTP socket
- frame decoder
- outbound sender/writer queue
- connection-local TCP/UDP resources
- current attachment handle if it is attached to a retained session

Rules:

- Workers enqueue outbound frames through the connection writer queue.
- Closing the connection closes the queue before joining the writer.
- Sends after close fail deterministically.
- Connection close detaches or closes the current session through the session
  API, not by directly tearing down retained resources.

### Resource

Retained listeners, UDP binds, and TCP streams each own:

- socket handle
- close state
- condition-variable wakeups
- budget lease

Target resource state machine:

```text
Open -> Closing -> Closed
```

Rules:

- `close()` is idempotent.
- Closing releases the budget exactly once.
- Closing wakes any worker blocked on that resource.
- The resource does not own `PortTunnelService`.

### Budget State

Move port-forward counters into a small shared budget state object, for example
`PortTunnelBudgetState`, owned by the service but shareable by leases.

Budget leases should hold only the budget state needed to decrement counters,
not a weak or strong pointer to the full service. This avoids service
destruction or resurrection during ordinary resource close.

## Phased Task Plan

Each phase should be one or more focused commits. Commit after each completed
task.

### Phase 1: Document And Test Exit Drain Semantics

Tasks:

- Add focused C++ session-store tests for idle grace, max grace, and descendant
  termination after grace.
- Add a short code comment near the C++ constants describing the Rust-aligned
  behavior.
- Verify POSIX and Windows XP build/test paths.

Expected result:

- The recently fixed output-drain policy is protected by deterministic tests.
- Future changes to grace values have a clear contract.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

### Phase 2: Extract Port-Tunnel Budget State

Tasks:

- Introduce `PortTunnelBudgetState` for worker, retained session, retained
  listener, UDP bind, and active TCP stream counters.
- Make budget leases refer to budget state instead of `PortTunnelService`.
- Preserve the existing public limit errors and counter names in abort logs.
- Add regression coverage for closing retained resources after the service
  external owner is dropped.

Expected result:

- Closing a budgeted resource cannot trigger `PortTunnelService` destruction or
  reentrant service lifecycle behavior.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

### Phase 3: Make Service Shutdown A State Machine

Tasks:

- Add explicit service lifecycle state.
- Make `shutdown()` idempotent and safe when called from any participating
  thread.
- Ensure destructor cleanup only drives the same state machine and does not
  introduce a separate teardown path.
- Add tests for repeated shutdown, shutdown from active connection paths, and
  shutdown with retained sessions.

Expected result:

- Service teardown has one order and one implementation.
- Repeated or concurrent shutdown requests are harmless.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
```

### Phase 4: Centralize Thread Ownership

Tasks:

- Extract a common tracked-thread owner for expiry scheduler, retained workers,
  and sender/writer threads where practical.
- Define one self-thread rule: a thread never joins itself; owner shutdown
  either detaches only at final self-destruction or defers join to another
  owner.
- Make thread handles transition to a consumed state after join, detach, or
  close.
- Add tests for shutdown while accept, UDP read, TCP read, and writer loops are
  blocked.

Expected result:

- Worker and scheduler lifetime behavior is consistent.
- Future lifecycle debugging does not require understanding separate thread
  ownership conventions for each subsystem.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

### Phase 5: Centralize Session Transitions

Tasks:

- Add explicit session state enum and transition helpers.
- Move attach, detach, close, expiry, retained-resource install, and
  retained-resource removal behind session methods.
- Return teardown actions from session transitions so sockets are closed outside
  session locks.
- Preserve resume and generation behavior.

Expected result:

- Session lifecycle becomes reviewable as one state machine.
- Expiry and explicit close share one terminal teardown path.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
```

### Phase 6: Resource-Owned Close Paths

Tasks:

- Move socket shutdown, close flags, wakeups, and budget release into resource
  methods for retained listeners, UDP binds, and TCP streams.
- Remove duplicate free-function close paths where the resource method is now
  authoritative.
- Add double-close and close-while-worker-blocked tests.

Expected result:

- Resources have one authoritative teardown path.
- Idempotence remains a safety property, not the primary coordination design.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
```

### Phase 7: Stress And Cross-Platform Gates

Tasks:

- Add a documented stress command for load-sensitive C++ daemon flakes.
- Consider adding a non-default make target that runs repeated
  `check-posix -j 8` loops.
- Run BSD make on a BSD target after port-forward lifecycle phases.
- Run the Windows XP compile and Wine-backed tests when available.

Expected result:

- The stress command that found the DragonFly bug becomes part of the standard
  debugging playbook.

Suggested stress command:

```sh
for i in `seq 10`; do
  make -C crates/remote-exec-daemon-cpp check-posix -j 8
  if [ $? -ne 0 ]; then
    break
  fi
done
```

Validation:

```sh
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

## Regression Test Inventory

Add or preserve deterministic coverage for:

- parent exits while descendant writes before idle grace
- parent exits while descendant writes repeatedly until max grace
- parent exits while descendant stays silent until termination
- service external owner drops while detached retained listener expires
- service external owner drops while detached UDP bind expires
- shutdown while retained TCP accept is blocked
- shutdown while retained UDP recv is blocked
- connection close while outbound control frames are queued
- worker-budget failure after socket bind but before worker start
- active stream close racing with tunnel close
- stale generation close after resume
- resume after detach while old workers unwind

## Commit Cadence

Use one commit per completed task or tightly coupled task group. Good commit
boundaries:

- one regression test plus the minimal fix it proves
- one ownership extraction with no behavior change
- one state-machine introduction with compatibility tests
- one cleanup commit that removes now-dead paths after tests pass

Avoid commits that combine:

- mechanical file moves with behavior changes
- POSIX and Windows lifecycle changes without a shared reason
- protocol-visible behavior changes with internal ownership cleanup

## Validation Matrix

Minimum for C++ daemon lifecycle changes:

```sh
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
git diff --check
```

When touching port-forward protocol behavior:

```sh
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
```

When touching shared Windows code:

```sh
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

When validating BSD behavior:

```sh
bmake -C crates/remote-exec-daemon-cpp check-posix
```

When validating load-sensitive behavior on a BSD target:

```sh
for i in `seq 10`; do
  make -C crates/remote-exec-daemon-cpp check-posix -j 8
  if [ $? -ne 0 ]; then
    break
  fi
done
```

## Success Criteria

- Session output drain behavior is specified and covered by boundary tests.
- Port-forward budget leases no longer depend on `PortTunnelService` lifetime.
- Service shutdown has an explicit state machine and documented order.
- Expiry, worker, and sender threads use consistent ownership and self-thread
  handling.
- Retained session transitions are centralized and reviewable.
- Retained resources own their close state, socket wakeups, and budget release.
- Known DragonFly/OpenBSD-style lifecycle races have deterministic regression
  tests.
- POSIX, BSD make, Windows XP, and Wine-backed gates pass where available.
