# C++ Hybrid Supervised Blocking Model Design

Status: approved design captured in writing

Date: 2026-05-06

References:

- `crates/remote-exec-daemon-cpp/src/server.cpp`
- `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
- `crates/remote-exec-daemon-cpp/src/basic_mutex.cpp`
- `crates/remote-exec-daemon-cpp/include/basic_mutex.h`
- `crates/remote-exec-daemon-cpp/include/session_store.h`
- `crates/remote-exec-daemon-cpp/include/port_forward.h`
- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`

## Goal

Strengthen the C++ daemon concurrency model without changing the public broker or
daemon behavior.

The daemon should keep a blocking I/O architecture that remains compatible with
Windows XP and older POSIX systems, but it should stop relying on detached
threads, ownerless blocking loops, and sleep-driven session polling as the main
coordination model.

The target model is hybrid supervised blocking:

- blocking sockets and process I/O stay in place
- thread ownership becomes explicit
- shutdown and close paths become coordinated
- waiting shifts from polling loops toward event-driven wakeups behind a
  portability wrapper

This is an internal architecture refactor, not a feature expansion.

## Scope

This design covers:

- HTTP listener and per-connection worker lifecycle
- explicit runtime ownership for active connection workers
- session output waiting and wakeup coordination
- cross-platform compatibility wrappers for signaling and timed waits
- port-forward lifecycle supervision
- shutdown ordering across listener, HTTP connections, exec sessions, and port
  forwards
- focused regression coverage for the new supervision model

This design does not cover:

- public MCP tool schema changes
- broker routing or broker session behavior changes
- TLS support for the C++ daemon
- HTTP/2, pipelining, or async reactor/event-loop migration
- PTY feature expansion
- replacing blocking process/session backends with non-blocking transports
- broad transfer feature changes beyond lifecycle supervision

## Constraints

The design must preserve the current platform envelope:

- Windows XP-compatible builds remain supported
- older POSIX systems remain supported
- the implementation cannot depend on IOCP, `epoll`, `kqueue`, modern C++
  threading primitives that are unavailable in the current build targets, or
  Vista-era Windows condition variables

Those constraints drive the architectural choice. The daemon should not attempt
Tokio-style async parity with the Rust implementation. It should instead make
its existing blocking model bounded, supervised, and stoppable.

## Problem Statement

The current C++ daemon already has concurrency, but it is unevenly controlled.

The HTTP server accepts clients in a blocking loop and spawns one detached OS
thread per accepted socket. Each worker then runs the entire keepalive request
loop synchronously on that thread. That is simple, but it leaves no central
owner for active client workers, no connection cap, and no coordinated reaping
path during shutdown.

Exec sessions have improved relative to the earlier global-lock bottleneck, but
the wait model still depends on active polling with `sleep_ms(...)` inside
session lifecycle code. That introduces unnecessary wakeups and keeps output
availability and completion detection coupled to retry intervals instead of real
signals from process output readers.

Port forwarding keeps a similarly blocking design. That part is acceptable for
the platform constraints, but the lifecycle model should be made consistent with
the rest of the daemon so blocked operations have one owner and one close path.

The main issue is therefore not "blocking I/O exists." The issue is that
blocking work is not yet consistently supervised.

## Decision Summary

### 1. Keep blocking I/O, but add supervision boundaries

The daemon should continue using blocking sockets, blocking process pipes, and
per-resource worker threads where needed.

The architectural change is to move ownership upward:

- the listener is owned by a runtime object
- active HTTP workers are owned by a connection manager
- exec session output readers are owned by the session store
- port-forward resources are owned by the port-forward store

This preserves portability while making shutdown and lifecycle management
explicit.

### 2. Replace detached HTTP workers with manager-owned workers

Accepted client sockets should no longer be handed to detached threads.

Instead, the listener thread should register each worker with a
`ConnectionManager` that:

- tracks active workers
- enforces a maximum active connection count
- records worker lifecycle state
- closes active client sockets during shutdown
- joins or reaps workers after they exit

The per-connection logic itself can remain blocking and largely preserve the
current `handle_client(...)` structure.

### 3. Replace sleep-driven session polling with blocking output pumps plus timed waits

Exec session coordination should move away from loops that repeatedly poll for
output and exit state with fixed sleeps.

Each live session should instead own:

- one or more output pump threads that block on process output reads
- a session buffer for accumulated unread output
- session exit and EOF state
- a wait primitive that can signal "new output or state change"

`start_command(...)` and `write_stdin(...)` then wait for one of three events:

- new output becomes available
- the process exits or reaches final EOF
- the configured deadline expires

This removes the current dependency on polling cadence while preserving the
existing RPC response shape.

### 4. Introduce one cross-platform wait/signal abstraction

The daemon should add a small portability layer for blocking waits and wakeups.

Recommended shape:

- keep `BasicMutex`
- add `BasicCondVar`
- expose wait, signal, broadcast, and timed-wait operations through a single
  compatibility API

Implementation guidance:

- POSIX: `pthread_mutex_t` plus `pthread_cond_t`
- Windows XP: event-backed implementation rather than Vista+ condition-variable
  APIs

Subsystem code should depend only on the wrapper API.

### 5. Keep port-forward operations blocking, but make close semantics explicit

`accept`, `recv`, and `send` in port-forward code may remain blocking.

What changes is the supervision model:

- each bind and TCP connection gets explicit lifecycle state
- lease expiry and explicit close mark the resource closing first
- socket closure is the mechanism that interrupts blocked operations
- map removal, lease cleanup, and socket close run through one consistent path

This aligns port forwarding with the rest of the daemon without requiring a
reactor.

### 6. Add one low-frequency maintenance thread

The daemon should add one maintenance thread owned by the runtime.

Its responsibilities are limited:

- reap finished connection workers that are no longer active
- sweep expired port-forward leases
- prune stale exited sessions if session-limit or cleanup policies require it

This avoids spreading cleanup across unrelated request paths.

## Runtime Architecture

The daemon should be organized around a small set of owner objects.

### `ServerRuntime`

`ServerRuntime` becomes the top-level owner for the process lifetime of the C++
daemon HTTP server. It owns:

- listener socket
- global shutdown flag
- `ConnectionManager`
- `SessionStore`
- `PortForwardStore`
- maintenance thread

`run_server(...)` should construct this runtime, enter the accept loop, and use
runtime-owned shutdown and cleanup paths instead of ad hoc detached-thread
behavior.

### `ConnectionManager`

`ConnectionManager` owns all active HTTP client workers.

Responsibilities:

- register an accepted client socket before worker start
- reject work when the connection cap is reached
- track worker state such as `starting`, `running`, `closing`, and `finished`
- reap finished workers and release capacity
- close active client sockets during shutdown
- join worker threads on POSIX and wait/close worker handles on Windows

The manager is the sole source of truth for active HTTP connection count.

### `HttpConnectionWorker`

Each worker owns exactly one client socket and runs the existing blocking HTTP
request loop.

Responsibilities:

- read and parse one request at a time
- route the request synchronously
- send the response synchronously
- decide whether to continue the keepalive loop
- exit on close-requested, timeout, parse failure, send failure, or
  manager-initiated socket close

The worker should remain intentionally narrow. It does request execution, not
global lifecycle policy.

### `SessionStore`

`SessionStore` remains the owner of live exec sessions, but it should also own
the session output pumps and wait/wakeup mechanics.

Each `LiveSession` should carry:

- process handle/backend object
- output buffer state
- exit and EOF state
- last-touched metadata
- `retired` and `closing` flags
- session-local mutex
- wait/signal primitive
- output pump handle ownership

### `PortForwardStore`

`PortForwardStore` remains the owner of bind and connection maps.

Its design change is not a new I/O model. The change is that every listener and
connection must have one consistent owner record, one close path, and one place
that performs state transition plus socket close plus lease cleanup.

## Threading Model

The supervised blocking model should use a small number of thread roles.

### Accept thread

One thread blocks in `accept(...)` on the listener socket.

Behavior:

- if shutdown begins, listener close breaks `accept(...)`
- if the connection manager is below limit, register and start a worker
- if the connection manager is at limit, close the accepted socket immediately
  and log capacity pressure

The accept thread does not route requests itself.

### Connection worker threads

Each active HTTP connection gets one worker thread.

That thread remains blocking and sequential for that client:

- read request head
- construct request body stream
- route request
- write response
- decide whether to keep the socket alive

The important difference from today is ownership. Workers are never detached and
never outlive the manager's knowledge of them.

### Session output pump threads

Each live exec session gets output pump thread ownership inside the session
store.

Depending on backend wiring, this may be one pump that handles merged output or
separate pumps for stdout and stderr. The design does not require changing the
current output surface, only how availability is coordinated.

Each pump:

- blocks on backend output reads
- appends output to the session buffer under the session mutex
- records EOF or fatal read state
- signals the session wait primitive after each meaningful state change

### Maintenance thread

One maintenance thread performs low-frequency reaping and sweeping.

It must not hold broad locks while doing blocking work. Its job is bookkeeping,
not heavy I/O.

## Session Wait Model

The current sleep-based session polling should be replaced with wait-driven
coordination.

### Session start

`start_command(...)` should:

1. launch the process
2. create and start output pump thread(s)
3. wait until output arrives, the process exits, or the configured startup
   deadline expires
4. if completed, return a finished response without storing a running session
5. if still running, insert the session into the store and return a running
   response

### Session write

`write_stdin(...)` should:

1. look up the target session
2. optionally write stdin under the session-local lock or session-owned process
   coordination boundary
3. wait until output arrives, the process exits, or the configured deadline
   expires
4. build either a running response or a finished response
5. retire and remove the session if it is completed

### Wakeup conditions

Waiters should wake on:

- output appended to the session buffer
- process exit detected
- output EOF detected
- session marked closing or retired
- timeout expiration

This allows the daemon to remain blocking internally while avoiding synthetic
poll intervals.

## Shutdown Model

Shutdown must be coordinated and idempotent.

Recommended order:

1. set the global shutdown flag
2. close the listener socket so the accept thread unwinds
3. mark the connection manager shutting down
4. close all active client sockets
5. stop accepting new port-forward work and close active bind and connection
   sockets
6. mark sessions closing, wake waiters, and terminate child processes as needed
7. join or reap worker, maintenance, and output pump threads

Important properties:

- closing a socket is the standard interruption mechanism for blocked network
  operations
- waiters must be signaled before or during shutdown so they do not sleep until
  timeout
- shutdown should be best-effort for process termination, but it should never
  depend on a detached worker eventually disappearing on its own

## HTTP Connection Management

The HTTP request path remains synchronous inside one worker.

### Connection admission

`ConnectionManager` should own a configurable `max_active_connections` limit.

When the limit is reached:

- accept the socket
- close it immediately without starting a worker
- record a log line that the daemon is under connection pressure

This is intentionally simpler than queueing at the daemon layer.

### Worker lifecycle

Each worker should be observable through explicit state:

- `starting`
- `running`
- `closing`
- `finished`

The manager should reclaim capacity only after the worker reaches `finished`.

### Idle connections

The worker should enforce one explicit internal idle-timeout constant in the
HTTP server layer rather than relying only on peer behavior.

### Error behavior

No new public behavior should be introduced.

The worker should keep existing request parsing, body framing, routing, and
response semantics. The concurrency refactor changes lifecycle control, not the
HTTP contract.

## Port Forwarding Design

Port forwarding remains blocking and resource-oriented.

### Listener binds

Each bind record should include:

- socket ownership
- lease metadata
- lifecycle state such as `open`, `closing`, or `closed`

Closing a bind should:

1. transition it to `closing`
2. remove it from the live map or otherwise prevent new lookups
3. close the socket
4. clean up lease tracking

### TCP connections

Each TCP connection should likewise have:

- socket ownership
- separate read and write coordination where needed
- lease metadata
- lifecycle state

Close requests, lease expiry, EOF, and shutdown should all flow through one
shared connection-close path.

### Blocking reads and writes

Blocked `recv(...)` and `send(...)` calls remain acceptable under this design,
provided that:

- closure is idempotent
- closure marks the resource state before closing the socket
- callers interpret close-during-blocking-operation as a normal lifecycle event,
  not only as a generic I/O failure

## Code Boundaries

### `crates/remote-exec-daemon-cpp/src/server.cpp`

- introduce runtime-owned listener lifecycle
- replace detached worker creation with manager registration
- route shutdown through runtime-owned close and reap paths

### `crates/remote-exec-daemon-cpp/src/http_connection.cpp`

- keep the blocking request loop
- make worker exit reasons explicit
- integrate idle timeout and manager-driven close handling

### `crates/remote-exec-daemon-cpp/include/basic_mutex.h`

- keep `BasicMutex`
- add the wait/signal abstraction interface used by sessions and potentially by
  connection reaping helpers

### `crates/remote-exec-daemon-cpp/src/basic_mutex.cpp`

- implement the POSIX and XP-compatible wait/signal wrapper

### `crates/remote-exec-daemon-cpp/include/session_store.h`

- extend `LiveSession` with output buffer state, wakeup primitive, closing
  state, and output pump ownership metadata

### `crates/remote-exec-daemon-cpp/src/session_store.cpp`

- replace sleep-based session polling with wait-driven coordination
- own output pump start, teardown, and wakeups
- preserve response formatting and session-limit behavior

### `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`

- keep the existing blocking read/write backend model
- expose the minimal hooks needed for pump threads and explicit close/EOF
  handling

### `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`

- keep the existing blocking backend model
- expose the minimal hooks needed for pump threads and explicit close/EOF
  handling while preserving XP compatibility

### `crates/remote-exec-daemon-cpp/src/port_forward.cpp`

- unify close and lease-expiry paths
- make lifecycle state transitions explicit
- align blocking operation interruption with the runtime shutdown model

## Error Handling

The refactor is internal-only, so public error surfaces should stay stable where
possible.

Requirements:

- stale session IDs still return the existing unknown-session behavior
- non-TTY stdin restrictions remain unchanged
- close-during-shutdown should not create new spurious internal-error surfaces
  when the operation is really a normal resource close
- same-session exec requests must remain serialized
- different sessions and different HTTP connections should no longer depend on
  detached or polling-based behavior for correctness

## Testing Strategy

Focused C++ daemon tests should cover the new supervision boundaries:

- wait primitive unit tests for signal, broadcast, and timeout behavior
- session-store tests proving output wakeups replace polling loops without
  changing response semantics
- session-store tests proving session teardown wakes blocked waiters
- HTTP tests for keepalive with manager-owned workers
- HTTP tests for connection-cap enforcement and orderly worker reaping
- shutdown tests with active client workers
- port-forward tests for close-during-accept, close-during-read, and
  lease-expiry cleanup

Relevant verification commands after implementation:

- `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- `make -C crates/remote-exec-daemon-cpp check-posix`
- `make -C crates/remote-exec-daemon-cpp check-windows-xp`

## Rejected Alternatives

### Rebuild the daemon around a cross-platform async event loop

This is rejected because XP and older POSIX support make a clean, low-risk
reactor design much more expensive than the problem requires.

It would introduce platform-specific complexity across:

- socket multiplexing
- wakeup semantics
- process I/O integration
- shutdown ordering

without changing the public daemon feature set.

### Keep detached worker threads and improve only session polling

This is rejected because it would solve only part of the lifecycle problem.

Detached HTTP workers still leave the daemon without:

- bounded active connection count
- deterministic worker reaping
- centralized shutdown participation

### Keep sleep-based polling and just tune the intervals

This is rejected because the core problem is the coordination model, not the
exact delay values.

Shorter sleeps waste more CPU. Longer sleeps make response timing less crisp.
Neither option gives the daemon a real signal path for "output arrived" or
"session is shutting down."
