# C++ Exec Concurrency And Robustness Design

Status: approved design captured in writing

Date: 2026-05-02

References:

- `crates/remote-exec-daemon-cpp/src/server.cpp`
- `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- `crates/remote-exec-daemon/tests/exec_rpc/unix.rs`

## Goal

Polish the C++ daemon exec engine by fixing the current cross-session serialization bottleneck and tightening the process-session backends enough to make concurrent `exec_command` and `write_stdin` behavior defensible under load.

The main target is practical concurrency and robustness, not full Rust parity.

## Scope

This design covers only the following changes:

- remove accidental global serialization across unrelated exec sessions
- preserve same-session serialization for correctness
- tighten process stdin write behavior on Win32 and POSIX backends
- make session teardown and completion handling explicit under concurrent requests
- add targeted regression coverage for cross-session concurrency and core output behavior

This design does not cover:

- public RPC or broker API changes
- worker-thread or event-loop redesign inside the exec engine
- PTY feature expansion
- broad shell-policy parity work
- login/shell feature additions
- broker-side formatting or warning-surface changes

## Current Behavior Summary

The C++ daemon accepts each HTTP client on its own thread, but `SessionStore` currently holds one global mutex while it performs long polling and process I/O inside both `start_command(...)` and `write_stdin(...)`.

As a result:

- one slow `write_stdin` poll can block unrelated sessions
- one long `exec_command` startup poll can block unrelated sessions
- same-session correctness currently depends partly on this accidental coarse lock

The process backends also remain slightly under-defensive:

- the Win32 stdin path assumes one `WriteFile(...)` call is enough for the full request body
- backend write failures are not consistently normalized around stdin-closed semantics
- C++ coverage does not yet prove the same core exec robustness properties already covered in the Rust daemon tests

## Decision Summary

### 1. Split locking into store-level and session-level responsibilities

`SessionStore` keeps a global mutex only for map ownership and store-wide invariants:

- enforcing `max_open_sessions`
- inserting sessions
- looking up sessions by ID
- removing completed sessions
- enumerating sessions during destruction

Each `LiveSession` gets its own mutex. That session mutex serializes all operations that touch the underlying process state:

- `write_stdin(...)`
- polling output
- checking exit state
- flushing carry buffers
- final completion handling for that session

This allows different sessions to progress concurrently while keeping same-session operations serialized.

### 2. Do not hold the global store mutex across poll waits or process I/O

Long operations must happen outside the global store mutex:

- initial polling after process launch
- `write_stdin(...)` writes
- output polling loops
- best-effort termination during shutdown

This is the main behavior change. The server already uses per-client threads, so removing this lock bottleneck is what allows that existing concurrency model to become effective for exec traffic.

### 3. Keep same-session operations strictly serialized

Simply dropping the global lock during polling is not enough.

Without a session-local lock, the following races become possible:

- two concurrent polls on the same session
- one stdin write racing with another stdin write
- one request observing completion while another still mutates the same process buffers

The session-local lock is therefore part of the core correctness model, not an optional cleanup.

### 4. Make removal identity-safe after concurrent completion

When a request completes a session, it should reacquire the global lock and erase the session only if the map still points to the same session object for that ID.

This keeps removal logic explicit and future-proof against stale-handle mistakes.

Even though the current code does not replace a live session with another object under the same ID, the erase path should still be written in an identity-safe way.

### 5. Keep immediate-completion behavior unchanged

`start_command(...)` should preserve the existing external behavior:

- launch the process
- do an initial poll
- if the process already completed, return a finished response without inserting a live session into the store
- otherwise insert the session and return a running response

The difference is that the initial poll must happen without monopolizing the global store mutex.

### 6. Tighten backend stdin write behavior without changing the public contract

The Win32 backend should write stdin in a loop until the full buffer is sent or a real error occurs, matching the POSIX backend’s overall intent more closely.

The POSIX backend already loops on writes, so the main adjustment there is to make broken-pipe-style failures map cleanly into stdin-closed semantics when the session can no longer accept input.

This pass should not attempt to redesign backend I/O models or add richer PTY support.

## Data Flow

### `exec/start`

1. route handler validates request and resolves shell/workdir
2. `SessionStore` launches the process
3. `SessionStore` performs the initial poll while holding only the session-local lock
4. if the process already exited:
   - build a completed response
   - return it without inserting the session into the map
5. otherwise:
   - reacquire the global lock
   - enforce session-limit rules
   - insert the session into the map
   - return a running response

### `exec/write`

1. route handler looks up the session ID through `SessionStore`
2. `SessionStore` grabs a shared handle to the target session under the global lock
3. `SessionStore` releases the global lock
4. `SessionStore` acquires the session-local lock
5. it performs optional stdin write plus the configured poll
6. if still running:
   - return a running response
7. if completed:
   - build the completed response
   - reacquire the global lock
   - erase that session only if the stored pointer still matches the completed object
   - return the completed response

### destruction

1. acquire the global lock
2. snapshot the currently tracked session objects
3. clear the map
4. release the global lock
5. terminate each snapped session best-effort without holding the store mutex

## Code Boundaries

### `crates/remote-exec-daemon-cpp/src/session_store.cpp`

- add a mutex to `LiveSession`
- narrow the global lock scope around map access only
- keep per-session polling and writes under the session lock
- make completion/erase ordering explicit
- snapshot-and-terminate during destructor cleanup

### `crates/remote-exec-daemon-cpp/include/session_store.h`

- extend `LiveSession` as needed for the per-session mutex and any helper methods or comments required by the refactor

### `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`

- loop `WriteFile(...)` until the full input buffer is written
- surface closed-pipe-style failures through the existing `stdin_closed` behavior rather than a generic internal error when the session can no longer accept input

### `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`

- keep the current write loop
- tighten broken-pipe-style handling if needed so the existing `stdin_closed` behavior stays stable when the session can no longer accept input

### `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`

- add regression coverage for two unrelated live sessions progressing independently
- add coverage for late output drain before final completion
- add coverage for newline-preserving token truncation if the current tests do not already prove it directly

### `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`

- add a route-level concurrency regression that proves one session poll does not block another unrelated exec session on the same daemon

## Error Handling

- `Unknown daemon session` behavior remains unchanged for stale or removed session IDs.
- Non-TTY stdin writes that are intentionally unsupported remain `stdin_closed`, not generic internal errors.
- Backend write failures that mean the session can no longer accept stdin should also map to the same existing `stdin_closed` surface.
- Same-session concurrent requests must serialize rather than racing.
- Different-session requests should no longer block one another merely because they share one daemon process.
- Destructor cleanup remains best-effort. If a process resists termination, the daemon should still avoid deadlocking shutdown on the global store mutex.

## Testing Strategy

Targeted tests should drive this work in the C++ daemon crate first:

- `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

After the targeted regressions pass, run the relevant broader checks:

- `make -C crates/remote-exec-daemon-cpp check-posix`
- `make -C crates/remote-exec-daemon-cpp check-windows-xp`

This pass should stay focused on host POSIX proof for the concurrency behavior while preserving XP build correctness.

## Rejected Alternatives

### Release the global lock during polling without adding a session-local lock

This is rejected because it improves concurrency by removing the only current same-session guard.

It would leave the daemon vulnerable to races between:

- two concurrent polls on the same session
- a poll and a write on the same session
- completion handling and concurrent reads/writes

### Add dedicated worker threads or a queued state machine per exec session

This is rejected because it adds more moving parts than the daemon needs right now:

- more lifecycle machinery
- more synchronization complexity
- more XP/Win32 maintenance burden

The current request is to polish the existing engine, not replace it with a new architecture.

### Broaden this pass to shell-policy parity and feature expansion

This is rejected to keep the task focused.

Concurrency and core backend robustness are independent improvements and should land first before wider parity work.
