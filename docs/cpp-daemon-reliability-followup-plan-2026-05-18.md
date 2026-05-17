# C++ Daemon Reliability Follow-Up Plan

Date: 2026-05-18

Status: planning

Related:

- `docs/cpp-port-forward-lifecycle-rework-plan.md`
- `docs/cpp-daemon-lifecycle-hardening-plan-2026-05-18.md`
- Commit `cbdb467` - `Add C++ daemon POSIX stress target`

## Context

Recent C++ daemon work fixed several concrete lifecycle failures and added an
opt-in POSIX stress target. The bug pattern is broader than any single failing
test:

- DragonFly exposed load-sensitive session-output loss after process exit.
- OpenBSD exposed port-forward teardown races and participating-thread
  shutdown hazards.
- Local refactoring exposed a deadlock risk in daemon lifecycle code.
- Stress runs are now part of the debugging path, but not every flaky race has
  a deterministic regression test yet.

The previous lifecycle plan focused mainly on port-forward ownership, shutdown,
thread consumption, retained sessions, and resource-owned close paths. This
follow-up plan broadens the reliability work to the rest of the C++ daemon
without reopening the public protocol or doing an unbounded rewrite.

## Goals

- Keep the C++ daemon C++11-compatible and Windows XP-compatible.
- Preserve the broker-daemon HTTP contract and v4 port-forward wire protocol.
- Make daemon lifecycle boundaries explicit across process sessions, HTTP
  connections, route handlers, test harnesses, and server shutdown.
- Convert high-signal flakes into deterministic tests with bounded timeouts and
  synchronization gates.
- Make stress, debugger, and cross-platform validation commands repeatable.
- Keep each task small enough to commit independently.

## Non-Goals

- Do not redesign the public `forward_ports`, exec, transfer, patch, or image
  API.
- Do not change the v4 tunnel frame format.
- Do not require C++14 or newer.
- Do not remove Windows XP-compatible build support.
- Do not merge the Rust and C++ daemon implementations.
- Do not use stress tests as a substitute for deterministic regression tests.

## Working Principles

- Commit after each completed task.
- Add or tighten tests before removing old lifecycle paths.
- Prefer explicit state transitions over destructor-driven coordination.
- Avoid blocking joins, socket close, or callbacks while holding shared daemon
  state locks.
- Keep platform-specific behavior isolated behind existing POSIX and Win32
  seams.
- When a race is found under load, add a deterministic reproduction if the
  daemon or harness can expose a practical synchronization point.
- Run under Wine when a task needs Windows execution coverage and Wine is
  available.

## Failure Themes To Retire

### Late Output And Session Exit Races

The daemon must keep reading command output after the parent exits while
descendants still hold inherited pipes. A fixed short delay is not a stable
contract under load.

Desired end state:

- Session terminal state records whether output drain ended by idle grace, max
  grace, EOF, explicit close, or forced descendant termination.
- Tests cover output before idle grace, repeated output until max grace, silent
  descendants, and explicit session close.
- POSIX and Win32 behavior remain intentionally aligned where the public
  contract is shared.

### Blocking Test Harness Reads

Test helpers that call blocking reads without protocol-aware deadlines can hide
the real failure by hanging after the daemon has already violated a contract.

Desired end state:

- Tunnel-frame test helpers use bounded waits with useful failure diagnostics.
- Tests report the last observed frame, socket state, and expected transition.
- Harness timeouts are long enough for slow BSD hosts but short enough to avoid
  wasting debugging cycles.

### Cross-Thread Shutdown And Self-Join Hazards

Port-forwarding exposed the highest-risk cases, but the same class of bug can
appear anywhere a worker, server, route handler, or connection object can become
the last owner of a runtime object.

Desired end state:

- Every daemon-owned thread has one owner and one documented shutdown path.
- No participating thread is required to join itself.
- Shutdown can be requested from route code, connection code, service code, or
  destructor cleanup without selecting a different teardown algorithm.

### Socket And Connection Lifecycle Drift

The C++ daemon has platform-specific socket handling and HTTP upgrade behavior.
Small differences in close, shutdown, wakeup, and EOF handling are plausible
sources of BSD-only flakes.

Desired end state:

- POSIX socket close paths are reviewed for `shutdown()`, `close()`, wakeups,
  and blocked worker behavior.
- Win32 socket close paths preserve XP-compatible behavior.
- HTTP connection shutdown and port-tunnel upgrade handoff have explicit owner
  transfer points.

### Sparse Diagnostics Under Stress

Stress runs are useful only if a failure leaves enough evidence to identify the
race instead of requiring repeated debugger sessions.

Desired end state:

- Low-noise lifecycle logs can be enabled for session IDs, tunnel session IDs,
  stream IDs, thread start/stop, and close reasons.
- Assertions identify invariant names, IDs, and current lifecycle state.
- Stress commands and debugger attach commands are documented near the tests
  they support.

## Phased Task Plan

### Phase 1: Lifecycle Contract Inventory

Tasks:

- Inventory lifecycle owners for process sessions, session store entries,
  HTTP connections, server runtime, transfer route handlers, and port-tunnel
  services.
- Document each owner, terminal state, thread owner, and close trigger in a
  short source-adjacent comment or test helper note.
- Identify duplicated close paths that still exist after the port-forward
  hardening work.

Expected result:

- Reviewers can find the intended lifecycle owner before changing a subsystem.
- Remaining ambiguous ownership areas are listed before code changes begin.

Validation:

```sh
git diff --check
```

### Phase 2: Harden Test Harness Timeouts

Tasks:

- Replace unbounded tunnel-frame helper reads with deadline-aware helpers.
- Include the expected frame type, socket fd or handle, and current test phase
  in assertion failures.
- Add a small helper for retrying expected async transitions without blind
  sleeps.
- Preserve compatibility with BSD make and GNU make test entry points.

Expected result:

- A missing frame fails with actionable diagnostics instead of leaving a stuck
  test process.
- Stress failures point to the first violated contract.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
```

### Phase 3: Process Session Lifecycle Tests

Tasks:

- Add deterministic tests for parent exit with late descendant output.
- Add tests for stdin close, explicit session close, and process-tree cleanup.
- Verify terminal session state and output renderer behavior after close.
- Keep POSIX-specific process-tree assertions out of Win32-only code paths.

Expected result:

- Session-output races are covered by synchronization-driven tests rather than
  only by `check-posix -j` load.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

### Phase 4: Server And HTTP Connection Shutdown

Tasks:

- Trace server accept-loop shutdown, connection-manager shutdown, and route
  handler ownership.
- Ensure connection close wakes blocked request readers and response writers.
- Ensure upgraded tunnel connections transfer ownership exactly once from HTTP
  routing into port-tunnel connection handling.
- Add deterministic tests for server shutdown with an idle connection, a
  blocked request body, and an upgraded tunnel connection.

Expected result:

- Server shutdown no longer depends on timing-sensitive socket behavior.
- HTTP and tunnel ownership boundaries are explicit.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp test-host-server-runtime
make -C crates/remote-exec-daemon-cpp test-host-server-routes
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
```

### Phase 5: Lifecycle Diagnostics

Tasks:

- Add opt-in lifecycle log messages for session close reasons, tunnel close
  reasons, thread start/stop, and timeout decisions.
- Keep `REMOTE_EXEC_LOG=off` test output quiet by default.
- Add invariant assertions with IDs and state names where lifecycle state
  machines reject impossible transitions.
- Document debugger attach and stress usage for C++ daemon lifecycle failures.

Expected result:

- A rare BSD or Wine failure leaves enough context to identify the failing
  lifecycle transition.

Validation:

```sh
REMOTE_EXEC_LOG=off make -C crates/remote-exec-daemon-cpp check-posix
REMOTE_EXEC_LOG=debug make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

### Phase 6: Cross-Platform Stress Gates

Tasks:

- Run `stress-posix` on at least one Linux host and one BSD host.
- Run `bmake stress-posix` where BSD make is available.
- Run Windows XP compile coverage after shared C++ changes.
- Run Wine-backed Windows tests when the task requires Windows execution
  coverage and Wine is available.
- Capture any platform-specific failure as a deterministic regression test
  before making a broad cleanup.

Expected result:

- The stress command that found the DragonFly race is part of the standard
  reliability gate, but deterministic tests remain the source of truth.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp STRESS_RUNS=10 STRESS_JOBS=8 stress-posix
bmake -C crates/remote-exec-daemon-cpp STRESS_RUNS=10 STRESS_JOBS=8 stress-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

### Phase 7: Cleanup After Proven Tests

Tasks:

- Remove duplicated close helpers, redundant state booleans, and dead fallback
  paths only after deterministic tests cover their behavior.
- Collapse repeated POSIX/Win32 lifecycle patterns where the platform seam does
  not require different code.
- Keep GNU make, BSD make, and NMAKE entry points aligned for any new test
  target.

Expected result:

- The daemon has fewer lifecycle code paths and better regression coverage.
- Future fixes should not require debugger-first investigation for ordinary
  shutdown, close, or output-drain bugs.

Validation:

```sh
make -C crates/remote-exec-daemon-cpp check-posix
bmake -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
```

## Commit Cadence

Use one commit for each completed task or tightly coupled test-plus-fix pair.
Good boundaries:

- one deterministic regression test and the minimal harness change it needs
- one lifecycle owner inventory with no behavior change
- one shutdown or close-path correction with focused tests
- one diagnostics addition with quiet default test output
- one cleanup that removes code already made redundant by previous commits

Avoid commits that combine:

- docs-only planning with implementation
- POSIX socket behavior changes and Win32 behavior changes without a shared
  lifecycle reason
- public protocol changes with internal daemon cleanup
- stress-target changes with unrelated lifecycle fixes

## Validation Matrix

Minimum local checks for docs-only changes:

```sh
git diff --check
```

Minimum C++ daemon lifecycle checks:

```sh
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
git diff --check
```

BSD make path:

```sh
bmake -C crates/remote-exec-daemon-cpp check-posix
```

Windows XP-compatible path:

```sh
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Broker-facing port-forward contract:

```sh
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp
```

Load-sensitive gate:

```sh
make -C crates/remote-exec-daemon-cpp STRESS_RUNS=10 STRESS_JOBS=8 stress-posix
```

## Success Criteria

- Test harness reads have bounded waits and useful diagnostics.
- Process-session output drain and terminal state behavior are covered by
  deterministic tests.
- Server runtime and HTTP connection shutdown have explicit ownership and wakeup
  behavior.
- Port-forward lifecycle diagnostics identify close reasons and state names.
- Stress commands run through GNU make and BSD make entry points.
- Windows XP-compatible compile coverage remains green for shared C++ changes.
- Known OpenBSD and DragonFly-style lifecycle failures have deterministic
  regression tests or documented stress gates when deterministic reproduction is
  not practical.
- Cleanup removes obsolete lifecycle paths without changing public behavior.
