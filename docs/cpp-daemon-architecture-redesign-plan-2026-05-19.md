# C++ Daemon Architecture Redesign Plan

Date: 2026-05-19

Status: planning

Scope: `crates/remote-exec-daemon-cpp/`

Related:

- `docs/cpp-daemon-architecture-audit-2026-05-19.md`
- `docs/cpp-daemon-lifecycle-hardening-plan-2026-05-18.md`
- `docs/cpp-daemon-reliability-followup-plan-2026-05-18.md`
- `docs/cpp-port-forward-lifecycle-rework-plan.md`

## Purpose

This document turns the C++ daemon architecture review into a concrete redesign
plan.

The goal is not a rewrite, and it is not a shift away from the current
concurrency model. The goal is to simplify the daemon so that the code becomes
easier to reason about, less dependent on call-site discipline, and less likely
to fail on BSD and other POSIX systems.

The highest-priority concerns are:

- the two BSD port-forwarding bugs already observed during testing,
- the broader port-forward lifecycle and ownership complexity,
- the transfer sandbox and partial-stream correctness issues,
- the need to sweep every syscall path that can be interrupted by `EINTR`,
- and the general problem of platform-specific behavior leaking into feature
  code.

Non-goals are equally important:

- do not change the thread-based concurrency model,
- do not introduce `openat`,
- do not change the broker-daemon public contract unless a separate contract
  task explicitly requires it,
- do not remove Windows XP-compatible build support,
- do not spend design effort on the `LC_ALL` / `LANG` issue,
- do not prioritize the IPv4-only listener limitation in this plan.

## Working Rules

- Preserve the daemon as a C++11 codebase.
- Preserve the current make and NMake entry points.
- Preserve v4 port-forward semantics.
- Preserve public broker-owned identifiers.
- Keep Rust and C++ behavior aligned where the shared contract overlaps.
- Commit after each completed task when implementation work begins.
- Keep the architecture incremental. Each phase should leave the tree in a
  buildable, reviewable state.

## Design Summary

The redesign should reduce bug surface by making boundaries explicit:

- one layer owns OS and platform quirks,
- one layer owns runtime/thread lifecycle,
- one layer owns HTTP transport,
- one layer owns RPC decoding and routing,
- one layer owns path authorization and sandbox policy,
- one layer owns command/session execution,
- one layer owns archive transfer,
- one layer owns port-forward lifecycle and protocol state,
- one layer owns capability reporting.

The main design principle is simple:

> feature code should describe behavior, while platform/runtime code should
> absorb system-specific complexity.

That gives the daemon a smaller number of places where BSD, Win32, or POSIX
differences can hide.

## Core Architectural Problems To Solve

### 1. System Calls Are Too Spread Out

The current codebase has raw or near-raw access to blocking I/O, sockets,
process waits, mutex waits, and close behavior across multiple feature files.
That makes it too easy to forget one of:

- `EINTR` retry,
- monotonic timeout handling,
- close-on-exec / handle inheritance,
- nonblocking send pressure,
- signal-safe shutdown,
- or platform feature availability.

The redesign should move those concerns into a narrow platform layer.

### 2. Port-Forward Lifecycle Is Too Distributed

The port-forward implementation currently mixes:

- retained session ownership,
- tunnel connection ownership,
- stream ownership,
- UDP peer state,
- expiry,
- send queue policy,
- and shutdown coordination.

That is the right place for bugs to appear on BSD, because the code must keep
working under reconnect, worker limits, and partial shutdown.

The redesign should convert port forwarding into explicit state machines with
clear ownership and close ordering.

### 3. Transfer Authorization Is Not Materialized Enough

The audit already showed the important issue:

- root-level authorization is not enough for recursive export/import,
- every materialized path needs its own authorization check,
- and streaming transfer needs stricter failure semantics.

The redesign should make path authorization and archive materialization a
first-class subsystem, not a set of helper checks scattered across export and
import code.

### 4. Partial Failure Is Too Easy To Report As Success

The daemon should not be able to complete a transfer response while internally
failing partway through the work.

That means the design has to decide, up front, whether a route is:

- preflightable before headers are sent,
- streamable with explicit failure signaling,
- or required to fail before response commitment.

### 5. Capability Detection Is Not Centralized

BSD and older POSIX targets differ in APIs the code currently assumes are
present.

The daemon should report capability truthfully at startup or first use, rather
than letting feature code rediscover platform support in ad hoc ways.

## Target Architecture

### `platform/`

This layer is the only place that should directly touch the lowest-level OS
APIs.

Responsibilities:

- file-descriptor and handle wrappers,
- socket wrappers,
- pipe and PTY wrappers,
- thread and condition-variable wrappers,
- deadline and monotonic-time wrappers,
- retry helpers for interrupted blocking operations,
- signal-aware close and shutdown helpers,
- nonblocking I/O helpers,
- process and child-wait helpers.

Design rule:

- feature modules should never call raw `read`, `write`, `recv`, `send`,
  `accept`, `connect`, `poll`, `select`, `waitpid`, `pthread_*`, or Win32
  wait/process primitives directly if a wrapper exists.

This is the place where the EINTR sweep should live. The point is to avoid
sprinkling ad hoc retry loops throughout the daemon.

### `runtime/`

This layer owns daemon lifecycle and thread supervision.

Responsibilities:

- worker creation and joining,
- shutdown propagation,
- bounded worker accounting,
- supervision of long-lived runtime objects,
- safe self-thread behavior,
- common helpers for "wait until ready or deadline" patterns.

Design rule:

- every daemon-owned thread should have one owner and one documented shutdown
  path.

### `http/`

HTTP should be a transport boundary, not a feature layer.

Responsibilities:

- request parsing,
- header normalization,
- body streaming,
- response writing,
- upgrade handling,
- connection close and half-close policy,
- stream termination.

Design rule:

- route handlers should not own HTTP details beyond the smallest necessary
  stream interaction.

### `rpc/`

RPC should decode requests, validate inputs, and translate feature results into
the broker-daemon contract.

Responsibilities:

- route table,
- request and response shaping,
- typed errors,
- public capability reporting,
- translation from daemon internals into public contract fields.

Design rule:

- feature modules should return typed daemon errors rather than inventing
  their own transport-specific output conventions.

### `sandbox_path/`

This layer owns all path normalization and allow/deny evaluation.

Responsibilities:

- canonical path policy,
- recursive authorization of materialized children,
- symlink policy,
- readable/writeable path checks,
- sandbox-aware deletion policy.

Design rule:

- every path that will actually be opened, removed, copied, or materialized
  should be checked as that path, not just as the original request root.

Important constraint:

- because the daemon intentionally does not use `openat`, the design cannot
  eliminate TOCTOU risk completely. The goal is to make authorization complete
  and consistent within the chosen model, not to claim race-free filesystem
  security.

### `exec/`

This layer owns command/session execution.

Responsibilities:

- process creation backends,
- PTY handling,
- stdio pipe handling,
- session store ownership,
- output drain semantics,
- child reaping,
- process-tree shutdown policy,
- Win32 and POSIX backend differences.

Design rule:

- session ownership and process ownership should be explicit and separate from
  HTTP/RPC ownership.

### `transfer/`

This layer owns archive import/export and file movement.

Responsibilities:

- archive parsing and emission,
- source planning,
- destination planning,
- recursive authorization,
- temporary-file handling,
- rename/commit behavior,
- strict EOF and terminator validation,
- failure cleanup.

Design rule:

- archive codec, filesystem policy, and transport streaming should be separate
  concerns.

### `port_forward/`

This layer owns v4 tunnel state, retained resources, and per-connection
behavior.

Responsibilities:

- frame codec,
- retained session state,
- upgraded connection handling,
- retained listener and UDP bind ownership,
- active TCP stream ownership,
- queue policy,
- reconnect and detach semantics,
- shutdown and expiry.

Design rule:

- protocol parsing should not directly mutate global state; it should feed a
  session state machine.

### `capabilities/`

This layer owns feature detection and capability reporting.

Responsibilities:

- PTY support,
- port-forward support,
- protocol version support,
- known POSIX/BSD feature availability,
- Windows XP constraints where relevant.

Design rule:

- capability truth should be computed centrally and reported consistently.

## EINTR And Blocking-Syscall Strategy

This is a separate design concern because it affects the whole daemon.

The daemon should not handle `EINTR` as a one-off edge case in feature code.
Instead, the platform layer should provide wrappers that encode the retry
policy in one place.

### Operations That Need Central Handling

The wrappers should cover at least:

- `read` and `write`,
- `recv` and `send`,
- `accept` and `connect`,
- `poll` and `select`,
- `waitpid`,
- `pthread_cond_wait` and timed waits where applicable,
- signal-aware sleep or deadline wait helpers,
- any socket or I/O loop that can be interrupted by a signal.

### Policy Rules

- Use monotonic deadlines, not repeated relative timeouts, for loops that can
  be interrupted.
- Retry `EINTR` at the platform boundary where doing so is safe and intended.
- Do not retry `close` blindly after `EINTR`; treat close ownership as consumed
  and let the wrapper decide how to report the outcome.
- Do not make each feature file rediscover the retry rule.
- Prefer wrappers that return a small typed result such as `ok`, `timed out`,
  `closed`, or `interrupted` rather than forcing every caller to map raw errno
  values itself.

### BSD / POSIX Relevance

This strategy matters because BSD hosts often expose more of the edge cases
that Linux hides during casual testing:

- interrupted socket reads,
- interrupted poll loops,
- condition-variable clock support differences,
- PTY API differences,
- and different send-pressure behavior on retained sockets.

## Port-Forward Redesign

Port forwarding is the part of the daemon that most clearly needs a stateful
redesign.

### Problem Shape

The current implementation mixes:

- connection-level protocol handling,
- retained session state,
- listener lifetime,
- UDP peer lifetime,
- worker thread lifetime,
- sender queue lifetime,
- and expiry/shutdown rules.

This makes it hard to know which object should close what, on which thread,
and in what order.

### Target Shape

Use a small number of explicit owners:

- `PortTunnelService` owns global limits, shutdown state, and the session map.
- `PortTunnelSession` owns retained-session state, generation, expiry, and
  retained resources.
- `PortTunnelConnection` owns one live upgraded HTTP tunnel and the
  connection-local resources attached to it.
- resource objects own sockets, wakeups, and budget leases.
- worker/runtime objects own thread handles and joins.

### State Machines

The session should have explicit states such as:

- `New`,
- `Attached`,
- `Detached`,
- `Closing`,
- `Closed`.

The resource layer should have explicit states such as:

- `Open`,
- `Closing`,
- `Closed`.

The exact enum names are less important than the invariant:

- there should be one authoritative transition path for each lifecycle,
  rather than many helper functions that all partially close things.

### Queue Policy

Control frames, error frames, heartbeats, and data frames should all have a
bounded policy.

The design should not allow unlimited growth in the name of control traffic.
If the peer is stalled, the system should have a clear policy for what gets
dropped, what gets delayed, and what causes teardown.

### Reconnect Policy

Reconnect should retain only the resources the contract actually says are
retained.

Do not preserve:

- active TCP stream state,
- per-peer UDP connector state,
- or any other ephemeral state not explicitly part of the retained session
  contract.

Do preserve:

- listeners or binds that are meant to survive a disconnect,
- session identity and generation,
- and future listen-side traffic where the contract requires it.

### Port-Forward Tasks In Order

1. Centralize protocol codec and state transitions.
2. Split retained session ownership from connection ownership.
3. Make socket ownership move-only and resource-local.
4. Add bounded sender policy for control and error traffic.
5. Normalize shutdown and reconnect ordering.
6. Add deterministic tests for stale attachment, reconnect, and queue
   pressure.

## Transfer Redesign

Transfer needs to become two things instead of one:

- a planner/authorizer,
- and a streaming archive executor.

### Current Risk Pattern

The current audit showed three classes of failure:

- recursive sandbox checks are not applied deeply enough,
- streaming export can look successful after partial failure,
- interrupted import can leave partial output behind.

### Target Shape

Split transfer into:

- `planner` - enumerate source/destination work items and expected paths,
- `authorizer` - validate each materialized path against sandbox policy,
- `archive_reader` / `archive_writer` - strict codec logic,
- `exporter` - source walk and emission,
- `importer` - archive consumption and destination materialization,
- `fs_ops` - filesystem mutations and temp-file handling.

### Design Rules

- Every child path in a recursive walk must be individually authorized.
- Archive import should require strict end-of-archive validation.
- Truncated or interrupted input should fail, not look like a shorter success.
- Import should prefer temp-file-then-rename behavior where feasible.
- Export should not report success after an internal failure that invalidates
  the archive stream.
- Streaming semantics should be explicit at the route layer; do not let HTTP
  commitment and archive completion drift apart.

### Transfer Tasks In Order

1. Add a planner/authorizer split.
2. Make recursive authorization explicit for both export and import.
3. Enforce strict archive terminator validation.
4. Add temp-file cleanup for interrupted import.
5. Decide whether a streamed response needs a preflight stage or a protocol
   error signal before it can be considered successful.
6. Add regression tests for recursive deny, symlink traversal, truncated
   archive, and partial-file cleanup.

## Exec And Session Redesign

Execution is less visibly broken than port forwarding, but it still benefits
from the same architectural cleanup.

### Goals

- Make session ownership obvious.
- Make child reaping predictable.
- Make PTY support a capability rather than an assumption.
- Make output drain semantics explicit.
- Keep POSIX and Win32 backends aligned on the contract where they overlap.

### Design Rules

- The session store owns public session identity and session lifetime.
- Process backends own OS-specific creation and wait details.
- Output pumps own their pipes or PTY handles, not the session map.
- Child reaping should be handled through the platform layer, not ad hoc in
  each backend.

### Output Drain Policy

The current lifecycle plans around output drain should be folded into the
exec/session architecture:

- parent exit does not automatically mean output is finished,
- descendants may keep inherited pipes open,
- idle drain and max drain should be explicit policy values,
- terminal state should record why draining stopped.

## HTTP And RPC Boundary

HTTP and RPC should be kept thin.

### HTTP

The HTTP layer should manage:

- socket accept/close,
- request parse,
- body streaming,
- upgrade handoff,
- chunked response write,
- and connection lifetime.

It should not own business rules for transfer, exec, or port forwarding.

### RPC

The RPC layer should manage:

- request validation,
- route dispatch,
- typed error translation,
- and capability responses.

The route layer should not know how to implement transfer recursion or port
forwarding internals.

## BSD And POSIX Compatibility Watchlist

This plan assumes the daemon will continue to be validated on BSD and other
POSIX systems where the code has already shown differences.

Known compatibility areas that should be isolated in platform code:

- `pthread_condattr_setclock` availability,
- `O_CLOEXEC` and `SOCK_CLOEXEC` fallbacks,
- `pipe2` / `accept4` availability,
- PTY API differences,
- `MSG_NOSIGNAL` versus `SO_NOSIGPIPE`,
- interrupted socket reads and sends,
- `poll` / `select` timeout semantics,
- `close` behavior under interruption,
- `EAGAIN` behavior on nonblocking UDP send paths,
- monotonic deadline support.

The plan is not to remove portability risk entirely. The plan is to make the
portability surface small enough that the risk is visible and testable.

## Phased Implementation Plan

The implementation should proceed in small commits. Each task should be
separable enough that failures can be bisected cleanly.

### Phase 0: Architectural Guardrails

Tasks:

- define the raw-system-call boundary,
- document the centralized EINTR policy,
- document the no-`openat` constraint and its path-authority implications,
- add or update source-adjacent comments where lifecycle ownership is not
  obvious,
- identify feature files that still need to move behind platform wrappers.

Expected result:

- reviewers can tell which layer is allowed to touch OS primitives.

### Phase 1: Platform Wrapper Consolidation

Tasks:

- move repeated retry behavior into the platform layer,
- unify deadline-based waiting,
- centralize close-on-exec / inheritance fallbacks,
- unify socket wakeup and shutdown helpers,
- make condition-variable clock support a probe, not an assumption.

Expected result:

- feature modules stop carrying local retry logic for the same classes of
  interrupted calls.

### Phase 2: Port-Forward State and Ownership Rewrite

Tasks:

- split codec, session, connection, resource, sender, TCP, and UDP concerns,
- make the retained session the authoritative owner of retained resources,
- make the connection the authoritative owner of connection-local resources,
- add bounded queue policy for control traffic,
- formalize reconnect and detach transitions,
- add targeted regression tests for BSD-sensitive failure cases.

Expected result:

- the port-forward implementation is understandable as a set of state
  machines rather than a pile of interleaved helper functions.

### Phase 3: Transfer Planner, Authorization, And Streaming Rules

Tasks:

- add recursive child authorization,
- split transfer planning from archive execution,
- require strict terminator validation on import,
- add temp-file cleanup and safe commit paths,
- prevent partial export/import from looking successful.

Expected result:

- the transfer routes fail truthfully instead of completing with silent
  corruption or partial state.

### Phase 4: Exec/Session Cleanup

Tasks:

- centralize session ownership and process backend ownership,
- make output drain policy explicit,
- make PTY capability reporting truthful,
- align POSIX and Win32 backend behavior where the public contract overlaps.

Expected result:

- command-session behavior is easier to reason about under exit, drain, and
  child-process edge cases.

### Phase 5: HTTP/RPC Thinning

Tasks:

- reduce route handlers to decode/dispatch/encode,
- move any remaining feature-specific transport behavior behind feature
  modules,
- keep upgrade and streaming handoff points explicit.

Expected result:

- transport code and feature logic no longer co-own the same failure paths.

### Phase 6: Capability Reporting And Final Compatibility Audit

Tasks:

- centralize feature detection,
- report platform limitations truthfully,
- review BSD-specific code paths one more time,
- confirm the daemon still behaves as expected on the supported Windows XP
  build path.

Expected result:

- the daemon reports what it actually supports, rather than assuming the
  caller will discover limitations by failure.

## Validation Plan

Validation should be staged with each phase.

Minimum validation set for the redesign:

- `make -C crates/remote-exec-daemon-cpp check-posix`
- `bmake -C crates/remote-exec-daemon-cpp check-posix`
- `make -C crates/remote-exec-daemon-cpp all-windows-xp`
- focused C++ tests for:
  - port-forward reconnect and close ordering,
  - transfer recursive authorization,
  - transfer truncation / partial-file cleanup,
  - EINTR retry behavior,
  - timeout behavior under signal interruption,
  - PTY capability reporting,
  - session output drain behavior.

If a change touches the public contract or shared behavior, the corresponding
Rust-side and broker-side tests should also be reviewed so the two
implementations stay aligned.

## Acceptance Criteria

The redesign can be considered complete when:

- the daemon has a single documented place for low-level syscall retry
  behavior,
- the port-forward lifecycle is understandable through explicit state
  transitions,
- recursive transfer authorization is path-complete,
- streamed transfer failures are reported truthfully,
- interrupted or transient POSIX behavior no longer causes hidden feature
  divergence,
- BSD and other POSIX compatibility issues are isolated behind platform
  wrappers rather than feature logic,
- and the code remains C++11, thread-based, no-`openat`, and XP-compatible.

## Deferred Items

The following items are intentionally not part of this redesign plan:

- locale environment handling (`LC_ALL` / `LANG`),
- the IPv4-only daemon listener limitation,
- any change to the public broker-daemon API that is not needed to support the
  above redesign,
- any switch away from the current thread-based concurrency model.

## Final Note

This plan is meant to make future changes easier, not to hide complexity under
new abstractions.

The right result is a daemon where:

- platform quirks are centralized,
- ownership is obvious,
- lifecycle is explicit,
- failures are truthful,
- and BSD-specific bugs have far fewer places to hide.
