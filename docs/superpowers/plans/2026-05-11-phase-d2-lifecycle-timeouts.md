# Phase D2 Lifecycle Timeouts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve only Phase D2 lifecycle and timeout risks from `docs/CODE_AUDIT_ROUND3.md`, without planning or implementing D3-D4.

**Architecture:** Treat the audit as review input against the current tree, not as the live contract. D2 keeps the broker-owned public session ID model intact, adds bounded waiting around session locks, hardens process/session cleanup in Rust and C++, and extends the existing `write_stdin` flow to carry an optional PTY resize event instead of adding a new public tool. Broker daemon RPC timeout coverage is a regression task because the current `DaemonClient::post()` already wraps forwarded exec calls.

**Tech Stack:** Rust 2024 workspace with Tokio, Axum, `portable-pty`, and shared `remote-exec-proto` schemas; standalone C++11 daemon with POSIX process/session handling, plain HTTP routes, and v4 port-tunnel framing; existing Cargo integration tests and C++ make targets.

---

## Scope

Included from round-3 Phase D2:

- `#6`: C++ POSIX child exit handling does not reap sessions whose stdout is held open by descendants.
- `#7`: Rust `LiveSession` has no `Drop`, so spawned children can survive error/drop paths.
- `#8`: Rust `exec_write` waits indefinitely on a per-session lock.
- `#9`: PTY resize never propagates after session start.
- `#10`: Broker forwarded exec calls need end-to-end timeout regression coverage.
- `#12`: Rust transcript tail temporarily grows beyond its configured tail limit.
- `#14`: C++ `tunnel_open` JSON parse/type failures become `internal_error` instead of `invalid_port_tunnel`.

Explicitly excluded from this plan: D3 observability/operability and D4 test reliability. Do not add request correlation IDs, metrics, daemon SIGTERM handling, logging-level changes, patch audit trails, exit-code taxonomy, config-drift cleanup, stub-daemon fixture refactors, port-allocation rewrites, or CI matrix changes as part of D2.

Current-state notes:

- `#10` is already mostly fixed in current code: `crates/remote-exec-broker/src/daemon_client.rs` wraps `post()` in `tokio::time::timeout(self.request_timeout, ...)`, and `exec_start`/`exec_write` both call `post()`. The D2 task is to add endpoint-specific regression tests so this does not regress.
- `#9` is a public-interface change. Because `write_stdin` is the public continuation point for live sessions, D2 piggybacks resize on `write_stdin` through an optional `pty_size` object. This avoids adding a seventh public tool and keeps resize routed through the same public session ownership checks.
- The C++ daemon is part of the shared `/v1/exec/write` contract. It must accept the same optional `pty_size` field: POSIX C++ PTYs resize through `ioctl(TIOCSWINSZ)`, while Windows XP-compatible C++ builds return a typed unsupported error because `tty=true` is already unsupported there.

## File Structure

- `docs/superpowers/plans/2026-05-11-phase-d2-lifecycle-timeouts.md`: this D2-only implementation plan.
- `crates/remote-exec-host/src/exec/transcript.rs`: hard-cap transcript tail growth and add unit tests.
- `crates/remote-exec-host/src/exec/session/live.rs`: add `Drop` cleanup and PTY resize entrypoint.
- `crates/remote-exec-host/src/exec/session/child.rs`: add child-level PTY resize support for `portable-pty` and winpty handling.
- `crates/remote-exec-host/src/exec/session/mod.rs`: add Unix process-group drop regression coverage.
- `crates/remote-exec-host/src/exec/store.rs`: add timeout-aware session locking and tests.
- `crates/remote-exec-host/src/exec/handlers.rs`: use the timeout-aware lock in `exec_write`, validate resize requests, and map busy sessions to a typed RPC error.
- `crates/remote-exec-host/src/exec/support.rs`: route optional resize requests before stdin writes.
- `crates/remote-exec-proto/src/rpc/exec.rs`: add shared `ExecPtySize` and optional `ExecWriteRequest.pty_size`.
- `crates/remote-exec-proto/src/rpc.rs`: re-export `ExecPtySize` for broker, host, daemon, and public schema callers.
- `crates/remote-exec-proto/src/public.rs`: add optional `WriteStdinInput.pty_size`.
- `crates/remote-exec-proto/src/rpc/error.rs`: add typed wire codes for session lock timeout and invalid PTY size.
- `crates/remote-exec-broker/src/tools/exec.rs`: forward optional resize metadata and keep session ownership checks unchanged.
- `crates/remote-exec-broker/src/bin/remote_exec.rs`: add CLI flags for PTY resize and construct `WriteStdinInput.pty_size`.
- `crates/remote-exec-broker/src/daemon_client.rs`: add endpoint-specific broker exec timeout regression tests.
- `crates/remote-exec-broker/tests/mcp_exec/session.rs`: add public broker coverage for forwarding resize metadata.
- `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`: record/assert forwarded resize metadata in broker tests.
- `crates/remote-exec-daemon/tests/exec_rpc/mod.rs`: update shared request literals.
- `crates/remote-exec-daemon/tests/exec_rpc/unix.rs`: add Rust daemon PTY resize integration coverage and update request literals.
- `crates/remote-exec-daemon/tests/exec_rpc/windows.rs`: update Windows request literals for the new optional field.
- `crates/remote-exec-daemon-cpp/include/process_session.h`: add resize API and typed resize error for C++ daemon sessions.
- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`: implement POSIX PTY resize and route child reaping through the POSIX child reaper.
- `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`: return typed unsupported resize errors.
- `crates/remote-exec-daemon-cpp/include/posix_child_reaper.h`: declare the POSIX signal-driven child reaper.
- `crates/remote-exec-daemon-cpp/src/posix_child_reaper.cpp`: implement a SIGCHLD self-pipe reaper for registered session children.
- `crates/remote-exec-daemon-cpp/src/server.cpp`: install the POSIX child reaper before accepting connections.
- `crates/remote-exec-daemon-cpp/src/session_store.cpp`: pass optional resize fields through `write_stdin` before writing chars.
- `crates/remote-exec-daemon-cpp/include/session_store.h`: update the C++ `write_stdin` signature.
- `crates/remote-exec-daemon-cpp/src/server_route_exec.cpp`: parse optional `pty_size`, map validation errors to typed RPC errors, and pass it to `SessionStore`.
- `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`: type-check `TunnelOpen` metadata parsing.
- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`: add C++ POSIX resize coverage and zombie-reaping regression coverage.
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`: add route-level C++ resize error coverage.
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`: add malformed `TunnelOpen` metadata coverage.
- `crates/remote-exec-daemon-cpp/mk/sources.mk`: include the new POSIX child reaper source in POSIX and host-test source groups.
- `README.md`: document optional `write_stdin.pty_size` behavior and daemon parity.
- `crates/remote-exec-daemon-cpp/README.md`: document POSIX C++ PTY resize support and Windows XP-compatible unsupported behavior.
- `skills/using-remote-exec-mcp/SKILL.md`: document how agents should use `write_stdin.pty_size`.

---

### Task 1: Save The Phase D2 Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-d2-lifecycle-timeouts.md`
- Test/Verify: `git status --short docs/superpowers/plans/2026-05-11-phase-d2-lifecycle-timeouts.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked plan artifact only. The repo already tracks multiple files under `docs/superpowers/plans`, so the D2 plan follows that convention.

- [ ] **Step 1: Verify this plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-d2-lifecycle-timeouts.md`
Expected: command exits successfully.

- [ ] **Step 2: Review the plan heading and scope.**

Run: `sed -n '1,95p' docs/superpowers/plans/2026-05-11-phase-d2-lifecycle-timeouts.md`
Expected: output names Phase D2 only, includes the required agentic-worker header, includes all D2 audit items, and explicitly excludes D3-D4.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-d2-lifecycle-timeouts.md
git commit -m "docs: plan phase d2 audit fixes"
```

### Task 2: Hard-Cap Rust Transcript Tail Growth

**Finding:** D2 `#12`

**Files:**
- Modify: `crates/remote-exec-host/src/exec/transcript.rs`
- Test/Verify: `cargo test -p remote-exec-host transcript_tail`

**Testing approach:** TDD
Reason: The defect is isolated to one in-memory data structure with deterministic private state. Unit tests can prove both the large-single-chunk case and repeated-small-chunk case without spawning processes.

- [ ] **Step 1: Add failing unit tests for hard tail bounds.**

Append this test module to `crates/remote-exec-host/src/exec/transcript.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::TranscriptBuffer;

    #[test]
    fn transcript_tail_capacity_stays_at_tail_limit_for_large_chunk() {
        let mut transcript = TranscriptBuffer::new(1024);
        let bytes = vec![b'x'; 4096];

        transcript.push(&bytes);

        assert_eq!(transcript.head.len(), 512);
        assert_eq!(transcript.tail.len(), 512);
        assert!(
            transcript.tail.capacity() <= 512,
            "tail capacity exceeded tail limit: {}",
            transcript.tail.capacity()
        );
        assert_eq!(transcript.total, 4096);
    }

    #[test]
    fn transcript_tail_never_exceeds_tail_limit_across_small_chunks() {
        let mut transcript = TranscriptBuffer::new(10);

        transcript.push(b"0123");
        transcript.push(b"4567");
        transcript.push(b"89ab");

        assert_eq!(transcript.head, b"01234");
        assert_eq!(transcript.tail, b"789ab");
        assert_eq!(transcript.tail.len(), 5);
        assert_eq!(transcript.total, 12);
    }

    #[test]
    fn transcript_tail_handles_zero_limit_without_growth() {
        let mut transcript = TranscriptBuffer::new(0);

        transcript.push(b"abcdef");

        assert!(transcript.head.is_empty());
        assert!(transcript.tail.is_empty());
        assert_eq!(transcript.total, 6);
    }
}
```

- [ ] **Step 2: Run the focused test and confirm it fails.**

Run: `cargo test -p remote-exec-host transcript_tail`
Expected: `transcript_tail_capacity_stays_at_tail_limit_for_large_chunk` fails against the current implementation because `tail.extend_from_slice(bytes)` grows the allocation to the full incoming chunk before draining the length back down.

- [ ] **Step 3: Replace `TranscriptBuffer::push` tail maintenance with proactive truncation.**

Replace the tail section of `push` in `crates/remote-exec-host/src/exec/transcript.rs` with:

```rust
        if tail_limit == 0 {
            self.tail.clear();
            return;
        }

        if bytes.len() >= tail_limit {
            self.tail = bytes[bytes.len() - tail_limit..].to_vec();
            return;
        }

        let overflow = self
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(tail_limit);
        if overflow > 0 {
            self.tail.drain(..overflow);
        }
        self.tail.extend_from_slice(bytes);
```

Keep the existing `total` update and head capture before this block.

- [ ] **Step 4: Run focused verification.**

Run: `cargo test -p remote-exec-host transcript_tail`
Expected: all three transcript tail tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-host/src/exec/transcript.rs
git commit -m "fix: hard cap exec transcript tail"
```

### Task 3: Terminate Rust Live Sessions On Drop

**Finding:** D2 `#7`

**Files:**
- Modify: `crates/remote-exec-host/src/exec/session/live.rs`
- Modify: `crates/remote-exec-host/src/exec/session/mod.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host live_session_drop`
  - `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** TDD
Reason: The leak is observable through a Unix process-group descendant that keeps touching a marker after the `LiveSession` owner is dropped. The existing D1 termination test already proved explicit termination; this task adds the drop-path counterpart.

- [ ] **Step 1: Add a failing Unix drop-path regression test.**

Add this test next to `exec_session_termination_kills_pipe_process_group_descendants` in `crates/remote-exec-host/src/exec/session/mod.rs`:

```rust
    #[cfg(unix)]
    #[tokio::test]
    async fn live_session_drop_kills_pipe_process_group_descendants() {
        use std::time::{Duration, Instant};

        use crate::config::ProcessEnvironment;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let marker = tempdir.path().join("drop-descendant-marker");
        let script = format!(
            "trap 'exit 0' TERM; (trap 'exit 0' TERM; while :; do touch {}; sleep 0.05; done) & echo ready; while :; do sleep 1; done",
            marker.display()
        );
        let cmd = vec![TEST_SHELL.to_string(), "-c".to_string(), script];

        let mut session = super::spawn::spawn(
            &cmd,
            tempdir.path(),
            false,
            &ProcessEnvironment::capture_current(),
        )
        .expect("session should spawn");

        let output = session
            .wait_for_output(Duration::from_secs(2))
            .await
            .expect("wait should succeed");
        match output {
            super::live::OutputWait::Chunk(chunk) => assert!(chunk.contains("ready")),
            _ => panic!("expected ready output"),
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "descendant did not create marker");

        drop(session);
        let modified_after_drop = std::fs::metadata(&marker)
            .expect("marker metadata")
            .modified()
            .expect("marker modified time");
        tokio::time::sleep(Duration::from_millis(250)).await;
        let modified_later = std::fs::metadata(&marker)
            .expect("marker metadata after drop")
            .modified()
            .expect("marker modified time after drop");

        assert_eq!(
            modified_after_drop, modified_later,
            "descendant kept running after LiveSession drop"
        );
    }
```

- [ ] **Step 2: Run the focused test and confirm it fails.**

Run: `cargo test -p remote-exec-host live_session_drop`
Expected: the new test fails because `LiveSession` does not terminate the child on drop.

- [ ] **Step 3: Add `Drop` for `LiveSession`.**

Append this implementation to `crates/remote-exec-host/src/exec/session/live.rs` after the `impl LiveSession` block:

```rust
impl Drop for LiveSession {
    fn drop(&mut self) {
        if self.exit_code.is_none() {
            let _ = self.child.terminate();
        }
    }
}
```

This intentionally uses the existing synchronous `SessionChild::terminate()` helper. It does not call async code from `Drop`.

- [ ] **Step 4: Run focused verification.**

Run: `cargo test -p remote-exec-host live_session_drop`
Expected: the new drop-path test passes.

- [ ] **Step 5: Run daemon exec regression coverage.**

Run: `cargo test -p remote-exec-daemon --test exec_rpc`
Expected: all exec RPC tests pass; completed sessions should not be killed after their exit code is recorded.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src/exec/session/live.rs crates/remote-exec-host/src/exec/session/mod.rs
git commit -m "fix: terminate rust exec sessions on drop"
```

### Task 4: Bound Rust `exec_write` Session Lock Waits

**Finding:** D2 `#8`

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc/error.rs`
- Modify: `crates/remote-exec-host/src/exec/store.rs`
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto exec_session_lock_timeout`
  - `cargo test -p remote-exec-host session_lock_timeout`
  - `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** TDD for store behavior, targeted integration regression for daemon behavior
Reason: The indefinite wait starts in `SessionStore::lock`. A store-level timeout test is deterministic; daemon exec RPC tests ensure the handler still maps unknown sessions and stdin-closed sessions correctly after the new typed timeout path is added.

- [ ] **Step 1: Add a typed RPC code for lock timeout.**

In `crates/remote-exec-proto/src/rpc/error.rs`, add `ExecSessionLockTimeout` after `UnknownSession` in `RpcErrorCode`, map it to `"exec_session_lock_timeout"` in `wire_value`, and add this match arm in `from_wire_value`:

```rust
            "exec_session_lock_timeout" => Some(Self::ExecSessionLockTimeout),
```

Add this unit test to the existing `#[cfg(test)]` module:

```rust
    #[test]
    fn rpc_error_code_exec_session_lock_timeout_round_trips() {
        assert_eq!(
            RpcErrorCode::ExecSessionLockTimeout.wire_value(),
            "exec_session_lock_timeout"
        );
        assert_eq!(
            RpcErrorCode::from_wire_value("exec_session_lock_timeout"),
            Some(RpcErrorCode::ExecSessionLockTimeout)
        );
    }
```

- [ ] **Step 2: Run the proto test.**

Run: `cargo test -p remote-exec-proto exec_session_lock_timeout`
Expected: the new proto test passes after the enum mapping is complete.

- [ ] **Step 3: Add timeout-aware store API and tests.**

In `crates/remote-exec-host/src/exec/store.rs`, add this enum near `InsertOutcome`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLockError {
    UnknownSession,
    TimedOut,
}
```

Add this public method to `impl SessionStore`:

```rust
    pub async fn lock_with_timeout(
        &self,
        session_id: &str,
        timeout: std::time::Duration,
    ) -> Result<SessionLease, SessionLockError> {
        let session = self
            .inner
            .read()
            .await
            .get(session_id)
            .map(|entry| entry.session.clone())
            .ok_or(SessionLockError::UnknownSession)?;
        self.lock_if_current_with_timeout(session_id, session, timeout)
            .await
    }
```

Change `lock_if_current` so it delegates through a new private helper:

```rust
    async fn lock_if_current(
        &self,
        session_id: &str,
        session: SharedSession,
    ) -> Option<SessionLease> {
        self.lock_if_current_after_guard(session_id, session.clone(), session.lock_owned().await)
            .await
    }

    async fn lock_if_current_with_timeout(
        &self,
        session_id: &str,
        session: SharedSession,
        timeout: std::time::Duration,
    ) -> Result<SessionLease, SessionLockError> {
        let guard = tokio::time::timeout(timeout, session.clone().lock_owned())
            .await
            .map_err(|_| SessionLockError::TimedOut)?;
        self.lock_if_current_after_guard(session_id, session, guard)
            .await
            .ok_or(SessionLockError::UnknownSession)
    }

    async fn lock_if_current_after_guard(
        &self,
        session_id: &str,
        session: SharedSession,
        guard: OwnedMutexGuard<LiveSession>,
    ) -> Option<SessionLease> {
        let is_current = self
            .inner
            .read()
            .await
            .get(session_id)
            .is_some_and(|current| session_matches(current, &session));
        if is_current {
            self.touch_if_current(session_id, &session).await;
            Some(SessionLease {
                inner: self.inner.clone(),
                session_id: session_id.to_string(),
                session,
                guard,
            })
        } else {
            None
        }
    }
```

Add this test to the `store.rs` test module:

```rust
    #[tokio::test]
    async fn session_lock_timeout_returns_timed_out_for_busy_session() {
        let store = SessionStore::default();
        let session_id = "session-1";

        store
            .insert(
                session_id.to_string(),
                spawn_pipe_session(&sleep_script(2)),
            )
            .await;
        let lease = store.lock(session_id).await.expect("initial lease");

        let started = std::time::Instant::now();
        let result = store
            .lock_with_timeout(session_id, Duration::from_millis(50))
            .await;

        assert_eq!(result.err(), Some(super::SessionLockError::TimedOut));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "lock timeout took too long: {:?}",
            started.elapsed()
        );

        drop(lease);
        assert!(
            store
                .lock_with_timeout(session_id, Duration::from_secs(1))
                .await
                .is_ok(),
            "lock should succeed after the busy lease is dropped"
        );
    }
```

- [ ] **Step 4: Run the store test.**

Run: `cargo test -p remote-exec-host session_lock_timeout`
Expected: the new timeout test passes.

- [ ] **Step 5: Use the timeout-aware lock in `exec_write_local`.**

In `crates/remote-exec-host/src/exec/handlers.rs`, import `SessionLockError`, add a five-second lock timeout constant, and replace the current `state.sessions.lock(...).await` call:

```rust
use super::store::SessionLockError;

const EXEC_WRITE_SESSION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
```

```rust
    let session = match state
        .sessions
        .lock_with_timeout(&daemon_session_id, EXEC_WRITE_SESSION_LOCK_TIMEOUT)
        .await
    {
        Ok(session) => session,
        Err(SessionLockError::UnknownSession) => {
            return Err(logged_bad_request(
                RpcErrorCode::UnknownSession,
                "Unknown daemon session",
            ));
        }
        Err(SessionLockError::TimedOut) => {
            return Err(crate::error::rpc_error(
                409,
                RpcErrorCode::ExecSessionLockTimeout,
                format!(
                    "Timed out waiting for daemon session `{daemon_session_id}` lock"
                ),
            ));
        }
    };
```

- [ ] **Step 6: Run daemon exec regression coverage.**

Run: `cargo test -p remote-exec-daemon --test exec_rpc`
Expected: exec RPC tests pass and unknown-session behavior still returns `unknown_session`.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc/error.rs crates/remote-exec-host/src/exec/store.rs crates/remote-exec-host/src/exec/handlers.rs
git commit -m "fix: bound exec write session lock waits"
```

### Task 5: Add Optional PTY Resize To `write_stdin`

**Finding:** D2 `#9`

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc/exec.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-proto/src/rpc/error.rs`
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-host/src/exec/session/live.rs`
- Modify: `crates/remote-exec-host/src/exec/session/child.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/src/bin/remote_exec.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec/session.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Modify: `crates/remote-exec-daemon/tests/exec_rpc/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/exec_rpc/unix.rs`
- Modify: `crates/remote-exec-daemon/tests/exec_rpc/windows.rs`
- Modify: `crates/remote-exec-daemon/tests/windows_pty_debug.rs`
- Modify: `crates/remote-exec-daemon-cpp/include/process_session.h`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/session_store.h`
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_exec.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `README.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify:
  - `cargo test -p remote-exec-proto exec_write_request`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_cli`
  - `cargo test -p remote-exec-daemon --test exec_rpc`
  - `make -C crates/remote-exec-daemon-cpp test-host-session-store`
  - `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** public-schema regression plus daemon integration tests
Reason: Resize crosses the public MCP schema, broker forwarding, Rust local/daemon host runtime, and C++ plain-HTTP daemon. The highest-value tests are public broker forwarding and real PTY `stty size` checks on daemon backends that support PTYs.

- [ ] **Step 1: Add shared resize schema and error code.**

In `crates/remote-exec-proto/src/rpc/exec.rs`, add this type above `ExecWriteRequest`:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExecPtySize {
    pub rows: u16,
    pub cols: u16,
}
```

Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub pty_size: Option<ExecPtySize>,` to `ExecWriteRequest`.

In `crates/remote-exec-proto/src/rpc.rs`, add `ExecPtySize` to the `pub use exec::{...}` list:

```rust
pub use exec::{
    ExecCompletedResponse, ExecOutputResponse, ExecPtySize, ExecResponse, ExecRunningResponse,
    ExecStartRequest, ExecStartResponse, ExecWarning, ExecWriteRequest, ExecWriteResponse,
    WarningCode,
};
```

In `crates/remote-exec-proto/src/public.rs`, import `ExecPtySize` and add the same optional field to `WriteStdinInput`:

```rust
use crate::rpc::{ExecPtySize, ExecWarning, PortForwardProtocolVersion, TransferWarning};
```

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_size: Option<ExecPtySize>,
```

In `crates/remote-exec-proto/src/rpc/error.rs`, add `InvalidPtySize` after `TtyUnsupported`, map it to `"invalid_pty_size"`, and add this `from_wire_value` arm:

```rust
            "invalid_pty_size" => Some(Self::InvalidPtySize),
```

- [ ] **Step 2: Update proto tests and request literals.**

Update `exec_write_request_omits_none_fields` in `crates/remote-exec-proto/src/rpc/exec.rs` so the literal includes `pty_size: None`, then add:

```rust
    #[test]
    fn exec_write_request_serializes_pty_size() {
        let request = ExecWriteRequest {
            daemon_session_id: "daemon-session-1".to_string(),
            chars: String::new(),
            yield_time_ms: None,
            max_output_tokens: None,
            pty_size: Some(super::ExecPtySize { rows: 33, cols: 101 }),
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "daemon_session_id": "daemon-session-1",
                "chars": "",
                "pty_size": {
                    "rows": 33,
                    "cols": 101,
                },
            })
        );
    }
```

Run: `rg -n "ExecWriteRequest \\{" crates/remote-exec-daemon crates/remote-exec-broker crates/remote-exec-proto`
Expected: every existing Rust literal is visible. Add `pty_size: None` to literals that are not specifically testing resize.

- [ ] **Step 3: Run proto verification.**

Run: `cargo test -p remote-exec-proto exec_write_request`
Expected: request serialization tests pass.

- [ ] **Step 4: Implement Rust host resize support.**

In `crates/remote-exec-host/src/exec/session/child.rs`, add:

```rust
    pub(super) fn resize_pty(&mut self, size: remote_exec_proto::rpc::ExecPtySize) -> anyhow::Result<()> {
        anyhow::ensure!(size.rows > 0 && size.cols > 0, "PTY rows and cols must be greater than zero");
        match self {
            SessionChild::Pty(pty) => pty.master.resize(portable_pty::PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            }).map_err(Into::into),
            #[cfg(all(windows, feature = "winpty"))]
            SessionChild::Winpty(_) => anyhow::bail!("PTY resize is not supported by the winpty backend"),
            SessionChild::Pipe(_) => anyhow::bail!("PTY resize requires a tty session"),
        }
    }
```

In `crates/remote-exec-host/src/exec/session/live.rs`, add:

```rust
    pub async fn resize_pty(
        &mut self,
        size: remote_exec_proto::rpc::ExecPtySize,
    ) -> anyhow::Result<()> {
        self.child.resize_pty(size)
    }
```

In `crates/remote-exec-host/src/exec/support.rs`, add:

```rust
pub(super) async fn resize_pty(
    session: &mut session::LiveSession,
    size: remote_exec_proto::rpc::ExecPtySize,
) -> anyhow::Result<()> {
    session.resize_pty(size).await
}
```

In `crates/remote-exec-host/src/exec/handlers.rs`, before stdin-closed validation, validate and apply `req.pty_size`:

```rust
    if let Some(size) = req.pty_size {
        if size.rows == 0 || size.cols == 0 {
            return Err(logged_bad_request(
                RpcErrorCode::InvalidPtySize,
                "PTY rows and cols must be greater than zero",
            ));
        }
        super::support::resize_pty(&mut session, size)
            .await
            .map_err(|err| {
                logged_bad_request(RpcErrorCode::TtyUnsupported, err.to_string())
            })?;
    }
```

- [ ] **Step 5: Forward resize through the broker and CLI.**

In `crates/remote-exec-broker/src/tools/exec.rs`, include the field when building `ExecWriteRequest`:

```rust
                    pty_size: input.pty_size,
```

In `crates/remote-exec-broker/src/bin/remote_exec.rs`, import `ExecPtySize`, add these fields to `WriteStdinArgs`, and set `pty_size` in `write_stdin_input`:

```rust
    #[arg(long, help = "Resize PTY rows for this live session; requires --pty-cols.")]
    pty_rows: Option<u16>,

    #[arg(long, help = "Resize PTY columns for this live session; requires --pty-rows.")]
    pty_cols: Option<u16>,
```

```rust
        pty_size: write_stdin_pty_size(args.pty_rows, args.pty_cols)?,
```

Add this helper near `write_stdin_input`:

```rust
fn write_stdin_pty_size(rows: Option<u16>, cols: Option<u16>) -> anyhow::Result<Option<ExecPtySize>> {
    match (rows, cols) {
        (None, None) => Ok(None),
        (Some(rows), Some(cols)) if rows > 0 && cols > 0 => {
            Ok(Some(ExecPtySize { rows, cols }))
        }
        (Some(_), Some(_)) => anyhow::bail!("--pty-rows and --pty-cols must be greater than zero"),
        _ => anyhow::bail!("--pty-rows and --pty-cols must be provided together"),
    }
}
```

- [ ] **Step 6: Add broker forwarding coverage.**

In `crates/remote-exec-broker/tests/support/stub_daemon.rs`, add a stored copy of the most recent write request:

```rust
    pub(super) last_exec_write_request: Arc<Mutex<Option<ExecWriteRequest>>>,
```

Initialize it in `stub_daemon_state`:

```rust
        last_exec_write_request: Arc::new(Mutex::new(None)),
```

In `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`, record the request before behavior handling:

```rust
    assert_eq!(req.daemon_session_id, "daemon-session-1");
    *state.last_exec_write_request.lock().await = Some(req.clone());
```

In `crates/remote-exec-broker/tests/support/fixture.rs`, add this method to the existing `impl BrokerFixture` block:

```rust
    pub async fn last_exec_write_request(&self) -> Option<remote_exec_proto::rpc::ExecWriteRequest> {
        self.stub_state.last_exec_write_request.lock().await.clone()
    }
```

In `crates/remote-exec-broker/tests/mcp_exec/session.rs`, add:

```rust
#[tokio::test]
async fn write_stdin_forwards_pty_size_to_daemon_session() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let start = fixture
        .call_tool(
            "exec_command",
            &serde_json::json!({
                "target": "builder-a",
                "cmd": "sleep 30",
                "tty": true,
                "yield_time_ms": 250,
            }),
        )
        .await;
    let session_id = start
        .structured_content
        .as_ref()
        .and_then(|value| value.get("session_id"))
        .and_then(|value| value.as_str())
        .expect("public session id");

    fixture
        .call_tool(
            "write_stdin",
            &serde_json::json!({
                "session_id": session_id,
                "chars": "",
                "pty_size": {
                    "rows": 33,
                    "cols": 101,
                },
            }),
        )
        .await;

    let forwarded = fixture.last_exec_write_request().await.expect("write request");
    assert_eq!(
        forwarded.pty_size,
        Some(remote_exec_proto::rpc::ExecPtySize { rows: 33, cols: 101 })
    );
}
```

- [ ] **Step 7: Add Rust daemon PTY resize integration coverage.**

In `crates/remote-exec-daemon/tests/exec_rpc/unix.rs`, add:

```rust
#[tokio::test]
async fn exec_write_resizes_pty_before_polling_output() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "stty size; sleep 0.2; stty size; sleep 30".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;
    assert!(started.output().running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started.daemon_session_id().expect("live session").to_string(),
                chars: String::new(),
                yield_time_ms: Some(2_000),
                max_output_tokens: None,
                pty_size: Some(remote_exec_proto::rpc::ExecPtySize { rows: 33, cols: 101 }),
            },
        )
        .await;

    assert!(response.output().running);
    assert!(
        response.output().output.contains("33 101"),
        "PTY size output did not include resized dimensions: {:?}",
        response.output().output
    );
}
```

Add a second test that sends `pty_size: Some(ExecPtySize { rows: 0, cols: 80 })` and asserts RPC error code `"invalid_pty_size"`.

- [ ] **Step 8: Implement C++ POSIX resize support and route parsing.**

In `crates/remote-exec-daemon-cpp/include/process_session.h`, add:

```cpp
class ProcessPtyResizeUnsupportedError : public std::runtime_error {
public:
    explicit ProcessPtyResizeUnsupportedError(const std::string& message)
        : std::runtime_error(message) {}
};
```

and add a virtual method:

```cpp
    virtual void resize_pty(unsigned short rows, unsigned short cols) = 0;
```

In `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`, add a `bool tty_` member to `PosixProcessSession`, pass it from both constructor call sites, and implement:

```cpp
    void resize_pty(unsigned short rows, unsigned short cols) override {
        if (!tty_ || !input_write_.valid()) {
            throw ProcessPtyResizeUnsupportedError("PTY resize requires a tty session");
        }
        if (rows == 0U || cols == 0U) {
            throw ProcessPtyResizeUnsupportedError("PTY rows and cols must be greater than zero");
        }
        struct winsize size;
        std::memset(&size, 0, sizeof(size));
        size.ws_row = rows;
        size.ws_col = cols;
        if (ioctl(input_write_.get(), TIOCSWINSZ, &size) != 0) {
            throw std::runtime_error(std::string("ioctl(TIOCSWINSZ) failed: ") + std::strerror(errno));
        }
    }
```

In `crates/remote-exec-daemon-cpp/src/process_session_win32.cpp`, implement:

```cpp
    void resize_pty(unsigned short rows, unsigned short cols) override {
        (void)rows;
        (void)cols;
        throw ProcessPtyResizeUnsupportedError("PTY resize is not supported on this host");
    }
```

Update `SessionStore::write_stdin` in the header and source to accept `bool has_pty_size, unsigned short pty_rows, unsigned short pty_cols`. Inside the existing `operation_mutex_` block, before `write_stdin(chars)`, call:

```cpp
            if (has_pty_size) {
                if (pty_rows == 0U || pty_cols == 0U) {
                    throw ProcessPtyResizeUnsupportedError(
                        "PTY rows and cols must be greater than zero"
                    );
                }
                session->process->resize_pty(pty_rows, pty_cols);
            }
```

In `server_route_exec.cpp`, parse optional `pty_size` with typed validation:

```cpp
        bool has_pty_size = false;
        unsigned short pty_rows = 0U;
        unsigned short pty_cols = 0U;
        const Json::const_iterator pty_size_it = body.find("pty_size");
        if (pty_size_it != body.end() && !pty_size_it->is_null()) {
            try {
                const Json& pty_size = *pty_size_it;
                const unsigned long rows = pty_size.at("rows").get<unsigned long>();
                const unsigned long cols = pty_size.at("cols").get<unsigned long>();
                if (rows == 0UL || cols == 0UL || rows > 65535UL || cols > 65535UL) {
                    return make_rpc_error_response(
                        400,
                        "invalid_pty_size",
                        "PTY rows and cols must be between 1 and 65535"
                    );
                }
                has_pty_size = true;
                pty_rows = static_cast<unsigned short>(rows);
                pty_cols = static_cast<unsigned short>(cols);
            } catch (const Json::exception& ex) {
                return make_rpc_error_response(
                    400,
                    "invalid_pty_size",
                    std::string("invalid PTY size: ") + ex.what()
                );
            }
        }
```

Add a catch arm before `Json::exception` in `handle_exec_write`:

```cpp
    } catch (const ProcessPtyResizeUnsupportedError& ex) {
        log_message(LOG_WARN, "server", std::string("exec/write pty resize unsupported: ") + ex.what());
        write_rpc_error(response, 400, "tty_unsupported", ex.what());
```

- [ ] **Step 9: Add C++ resize tests.**

In `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`, extend `assert_stdin_and_tty_behavior` under the existing POSIX PTY branch with a command that prints `stty size` before and after resize:

```cpp
        const Json resize_running = start_test_command(
            store,
            "stty size; sleep 0.2; stty size; sleep 30",
            root.string(),
            shell,
            true,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        assert(resize_running.at("running").get<bool>());
        const Json resized = store.write_stdin(
            resize_running.at("daemon_session_id").get<std::string>(),
            "",
            true,
            2000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            true,
            33U,
            101U
        );
        assert(resized.at("running").get<bool>());
        assert(
            normalize_output(resized.at("output").get<std::string>()).find("33 101") !=
            std::string::npos
        );
```

Update every existing C++ `store.write_stdin(...)` call to pass `false, 0U, 0U` for the new resize arguments.

In `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`, add route-level coverage that posts `"pty_size": {"rows": 0, "cols": 80}` and asserts RPC code `"invalid_pty_size"`; add a non-TTY live session resize request and assert `"tty_unsupported"`.

- [ ] **Step 10: Update docs for the public resize contract.**

In `README.md`, add this bullet near the existing `write_stdin` semantics:

```md
- `write_stdin` accepts optional `pty_size: { "rows": N, "cols": N }` for live TTY sessions. The resize is applied before any `chars` are written, so a resize-only poll can omit `chars` or pass an empty string. Both values must be between 1 and 65535. Non-TTY sessions return `tty_unsupported`.
```

In `crates/remote-exec-daemon-cpp/README.md`, add:

```md
POSIX builds support `write_stdin.pty_size` for live `tty=true` sessions by applying `TIOCSWINSZ` to the PTY master. Windows XP-compatible builds continue to reject `tty=true`, so resize requests return the same typed unsupported-session error path.
```

In `skills/using-remote-exec-mcp/SKILL.md`, add to the `write_stdin` section:

```md
- For live TTY sessions, include `pty_size: { "rows": 33, "cols": 101 }` to resize before polling or writing. Omit `chars` for a resize-only poll. Do not send `pty_size` for non-TTY sessions.
```

- [ ] **Step 11: Run focused resize verification.**

Run:

```bash
cargo test -p remote-exec-proto exec_write_request
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-broker --test mcp_cli
cargo test -p remote-exec-daemon --test exec_rpc
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-routes
```

Expected: all commands pass.

- [ ] **Step 12: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc/exec.rs crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/rpc/error.rs crates/remote-exec-proto/src/public.rs crates/remote-exec-host/src/exec/session/live.rs crates/remote-exec-host/src/exec/session/child.rs crates/remote-exec-host/src/exec/support.rs crates/remote-exec-host/src/exec/handlers.rs crates/remote-exec-broker/src/tools/exec.rs crates/remote-exec-broker/src/bin/remote_exec.rs crates/remote-exec-broker/tests/mcp_exec/session.rs crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/support/stub_daemon_exec.rs crates/remote-exec-broker/tests/support/fixture.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs crates/remote-exec-daemon/tests/exec_rpc/mod.rs crates/remote-exec-daemon/tests/exec_rpc/unix.rs crates/remote-exec-daemon/tests/exec_rpc/windows.rs crates/remote-exec-daemon/tests/windows_pty_debug.rs crates/remote-exec-daemon-cpp/include/process_session.h crates/remote-exec-daemon-cpp/src/process_session_posix.cpp crates/remote-exec-daemon-cpp/src/process_session_win32.cpp crates/remote-exec-daemon-cpp/include/session_store.h crates/remote-exec-daemon-cpp/src/session_store.cpp crates/remote-exec-daemon-cpp/src/server_route_exec.cpp crates/remote-exec-daemon-cpp/tests/test_session_store.cpp crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp README.md crates/remote-exec-daemon-cpp/README.md skills/using-remote-exec-mcp/SKILL.md
git commit -m "feat: support pty resize through write stdin"
```

### Task 6: Add Broker Exec RPC Timeout Regression Tests

**Finding:** D2 `#10`

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Test/Verify: `cargo test -p remote-exec-broker daemon_exec_rpc_times_out`

**Testing approach:** characterization/regression test
Reason: Current code already routes `exec_start` and `exec_write` through `DaemonClient::post()`. Endpoint-specific tests prove future refactors do not bypass the existing request timeout.

- [ ] **Step 1: Add a helper for hung daemon responses.**

In the `#[cfg(test)]` module of `crates/remote-exec-broker/src/daemon_client.rs`, add:

```rust
    async fn hung_response_client(timeout: Duration) -> (DaemonClient, tokio::task::JoinHandle<()>) {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = DaemonClient {
            client: reqwest::Client::builder().build().unwrap(),
            target_name: "builder-a".to_string(),
            base_url: format!("http://{addr}"),
            authorization: None,
            request_timeout: timeout,
        };

        (client, server)
    }
```

Update `daemon_rpc_times_out_hung_response` to use this helper and keep its existing assertion for `/v1/target-info`.

- [ ] **Step 2: Add endpoint-specific exec timeout tests.**

Add:

```rust
    #[tokio::test]
    async fn daemon_exec_start_rpc_times_out_hung_response() {
        let (client, server) = hung_response_client(Duration::from_millis(50)).await;

        let err = client
            .exec_start(&ExecStartRequest {
                cmd: "sleep 30".to_string(),
                workdir: None,
                shell: None,
                tty: false,
                yield_time_ms: None,
                max_output_tokens: None,
                login: None,
            })
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("daemon rpc `/v1/exec/start` timed out after 50 ms"),
            "unexpected error: {err}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn daemon_exec_write_rpc_times_out_hung_response() {
        let (client, server) = hung_response_client(Duration::from_millis(50)).await;

        let err = client
            .exec_write(&ExecWriteRequest {
                daemon_session_id: "daemon-session-1".to_string(),
                chars: String::new(),
                yield_time_ms: None,
                max_output_tokens: None,
                pty_size: None,
            })
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("daemon rpc `/v1/exec/write` timed out after 50 ms"),
            "unexpected error: {err}"
        );
        server.abort();
    }
```

- [ ] **Step 3: Run focused verification.**

Run: `cargo test -p remote-exec-broker daemon_exec_rpc_times_out`
Expected: both endpoint-specific timeout tests pass and complete in under one second each.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-broker/src/daemon_client.rs
git commit -m "test: cover broker exec rpc timeouts"
```

### Task 7: Map C++ `TunnelOpen` Metadata Failures To `invalid_port_tunnel`

**Finding:** D2 `#14`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** TDD
Reason: This is a wire-level error mapping. A malformed frame can be sent through the existing streaming test harness and should produce an `Error` frame with code `invalid_port_tunnel`, not `internal_error`.

- [ ] **Step 1: Add malformed `TunnelOpen` tests.**

Add this helper near `assert_tunnel_rejects_frames_for_wrong_role_or_protocol` in `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`:

```cpp
static void assert_tunnel_open_metadata_error(
    AppState& state,
    const std::string& meta
) {
    UniqueSocket client_socket;
    std::thread server_thread;
    open_tunnel(state, &client_socket, &server_thread);

    PortTunnelFrame frame = empty_frame(PortTunnelFrameType::TunnelOpen, 0U);
    frame.meta = meta;
    send_tunnel_frame(client_socket.get(), frame);

    const PortTunnelFrame error = read_tunnel_frame(client_socket.get());
    assert(error.stream_id == 0U);
    assert_tunnel_error_code(error, "invalid_port_tunnel");

    close_tunnel(&client_socket, &server_thread);
}
```

Call it from `assert_tunnel_rejects_frames_for_wrong_role_or_protocol`:

```cpp
    assert_tunnel_open_metadata_error(state, "{not-json");
    assert_tunnel_open_metadata_error(
        state,
        Json{{"role", "listen"}, {"protocol", "tcp"}}.dump()
    );
    assert_tunnel_open_metadata_error(
        state,
        Json{{"role", 7}, {"protocol", "tcp"}, {"generation", 1ULL}}.dump()
    );
    assert_tunnel_open_metadata_error(
        state,
        Json{{"role", "listen"}, {"protocol", "tcp"}, {"generation", "bad"}}.dump()
    );
```

- [ ] **Step 2: Run the focused test and confirm it fails.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: the new malformed metadata cases fail because at least one malformed field is reported as `internal_error`.

- [ ] **Step 3: Add typed `TunnelOpen` metadata parsing.**

In `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`, add this helper above `PortTunnelConnection::tunnel_open`:

```cpp
struct TunnelOpenMetadata {
    std::string role;
    std::uint64_t generation;
    std::string protocol;
    bool has_resume_session_id;
    std::string resume_session_id;
};

TunnelOpenMetadata parse_tunnel_open_metadata(const PortTunnelFrame& frame) {
    try {
        const Json meta = Json::parse(frame.meta);
        TunnelOpenMetadata parsed;
        parsed.role = meta.at("role").get<std::string>();
        parsed.generation = meta.at("generation").get<std::uint64_t>();
        parsed.protocol = meta.at("protocol").get<std::string>();
        parsed.has_resume_session_id = false;
        if (meta.contains("resume_session_id") && !meta.at("resume_session_id").is_null()) {
            parsed.has_resume_session_id = true;
            parsed.resume_session_id = meta.at("resume_session_id").get<std::string>();
        }
        return parsed;
    } catch (const Json::exception& ex) {
        throw PortForwardError(
            400,
            "invalid_port_tunnel",
            std::string("invalid tunnel open metadata: ") + ex.what()
        );
    }
}
```

Then replace the direct parse block in `tunnel_open`:

```cpp
    const TunnelOpenMetadata meta = parse_tunnel_open_metadata(frame);
    const std::string role = meta.role;
    const std::uint64_t generation = meta.generation;
    const std::string protocol = meta.protocol;
```

and update resume handling:

```cpp
        if (meta.has_resume_session_id) {
            const std::string session_id = meta.resume_session_id;
```

- [ ] **Step 4: Run focused verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: streaming tests pass, including the malformed `TunnelOpen` metadata cases.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "fix: type tunnel open metadata errors"
```

### Task 8: Reap C++ POSIX Session Children Whose Output Stays Open

**Finding:** D2 `#6`

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/posix_child_reaper.h`
- Create: `crates/remote-exec-daemon-cpp/src/posix_child_reaper.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-session-store`
  - `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** integration-style C++ regression test
Reason: The leak is observable only when the direct session child exits while a descendant keeps stdout open, causing the output pump to block before it can call `has_exited`. A POSIX `/proc` test catches this on Linux without changing the public API.

- [ ] **Step 1: Add the POSIX child reaper interface.**

Create `crates/remote-exec-daemon-cpp/include/posix_child_reaper.h`:

```cpp
#pragma once

#ifndef _WIN32
#include <sys/types.h>

void install_posix_child_reaper();
void register_posix_child(pid_t pid);
void unregister_posix_child(pid_t pid);
bool take_reaped_posix_child(pid_t pid, int* status);

#endif
```

- [ ] **Step 2: Implement a registered-child SIGCHLD reaper.**

Create `crates/remote-exec-daemon-cpp/src/posix_child_reaper.cpp` with a self-pipe SIGCHLD handler. The handler only writes one byte to the pipe. The reaper thread copies registered PIDs, calls `waitpid(pid, &status, WNOHANG)` for those PIDs only, stores reaped statuses in a map, and never calls `waitpid(-1, ...)`.

Use this structure:

```cpp
#ifndef _WIN32

#include "posix_child_reaper.h"

#include <cerrno>
#include <csignal>
#include <cstring>
#include <map>
#include <set>
#include <stdexcept>
#include <thread>
#include <vector>

#include <fcntl.h>
#include <sys/select.h>
#include <sys/wait.h>
#include <unistd.h>

#include "basic_mutex.h"
#include "logging.h"

namespace {

BasicMutex g_mutex;
std::set<pid_t> g_registered;
std::map<pid_t, int> g_reaped;
int g_signal_pipe_read = -1;
int g_signal_pipe_write = -1;
bool g_installed = false;

void set_cloexec_nonblock(int fd) {
    const int fd_flags = fcntl(fd, F_GETFD, 0);
    if (fd_flags >= 0) {
        fcntl(fd, F_SETFD, fd_flags | FD_CLOEXEC);
    }
    const int status_flags = fcntl(fd, F_GETFL, 0);
    if (status_flags >= 0) {
        fcntl(fd, F_SETFL, status_flags | O_NONBLOCK);
    }
}

void sigchld_handler(int) {
    if (g_signal_pipe_write >= 0) {
        const unsigned char byte = 1U;
        (void)write(g_signal_pipe_write, &byte, 1U);
    }
}

std::vector<pid_t> registered_snapshot() {
    BasicLockGuard lock(g_mutex);
    return std::vector<pid_t>(g_registered.begin(), g_registered.end());
}

void record_reaped(pid_t pid, int status) {
    BasicLockGuard lock(g_mutex);
    if (g_registered.find(pid) != g_registered.end()) {
        g_reaped[pid] = status;
    }
}

void reap_registered_children() {
    const std::vector<pid_t> pids = registered_snapshot();
    for (std::size_t i = 0; i < pids.size(); ++i) {
        int status = 0;
        for (;;) {
            const pid_t result = waitpid(pids[i], &status, WNOHANG);
            if (result == pids[i]) {
                record_reaped(pids[i], status);
                break;
            }
            if (result == 0) {
                break;
            }
            if (result < 0 && errno == EINTR) {
                continue;
            }
            break;
        }
    }
}

void drain_signal_pipe() {
    unsigned char buffer[64];
    while (g_signal_pipe_read >= 0 && read(g_signal_pipe_read, buffer, sizeof(buffer)) > 0) {
    }
}

void reaper_loop() {
    for (;;) {
        fd_set read_fds;
        FD_ZERO(&read_fds);
        FD_SET(g_signal_pipe_read, &read_fds);
        timeval timeout;
        timeout.tv_sec = 1;
        timeout.tv_usec = 0;
        const int ready = select(g_signal_pipe_read + 1, &read_fds, NULL, NULL, &timeout);
        if (ready > 0) {
            drain_signal_pipe();
        }
        reap_registered_children();
    }
}

}  // namespace

void install_posix_child_reaper() {
    BasicLockGuard lock(g_mutex);
    if (g_installed) {
        return;
    }
    int fds[2];
    if (pipe(fds) != 0) {
        throw std::runtime_error(std::string("pipe(SIGCHLD) failed: ") + std::strerror(errno));
    }
    g_signal_pipe_read = fds[0];
    g_signal_pipe_write = fds[1];
    set_cloexec_nonblock(g_signal_pipe_read);
    set_cloexec_nonblock(g_signal_pipe_write);

    struct sigaction action;
    std::memset(&action, 0, sizeof(action));
    action.sa_handler = sigchld_handler;
    sigemptyset(&action.sa_mask);
    action.sa_flags = SA_RESTART | SA_NOCLDSTOP;
    if (sigaction(SIGCHLD, &action, NULL) != 0) {
        throw std::runtime_error(std::string("sigaction(SIGCHLD) failed: ") + std::strerror(errno));
    }

    std::thread(reaper_loop).detach();
    g_installed = true;
    log_message(LOG_INFO, "posix_child_reaper", "installed SIGCHLD child reaper");
}

void register_posix_child(pid_t pid) {
    BasicLockGuard lock(g_mutex);
    g_registered.insert(pid);
}

void unregister_posix_child(pid_t pid) {
    BasicLockGuard lock(g_mutex);
    g_registered.erase(pid);
    g_reaped.erase(pid);
}

bool take_reaped_posix_child(pid_t pid, int* status) {
    BasicLockGuard lock(g_mutex);
    std::map<pid_t, int>::iterator it = g_reaped.find(pid);
    if (it == g_reaped.end()) {
        return false;
    }
    *status = it->second;
    g_reaped.erase(it);
    g_registered.erase(pid);
    return true;
}

#endif
```

- [ ] **Step 3: Register POSIX process sessions with the reaper.**

In `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`, include `posix_child_reaper.h`. In `PosixProcessSession`:

- Call `register_posix_child(pid_)` in the constructor.
- Call `unregister_posix_child(pid_)` when `has_exited` records a status from direct `waitpid`.
- Check `take_reaped_posix_child(pid_, &status)` at the start of `has_exited`.
- Check `take_reaped_posix_child(pid_, &ignored_status)` in `terminate()` before blocking in `waitpid(pid_, ..., 0)`.
- Call `unregister_posix_child(pid_)` in `terminate()` when the direct wait succeeds or returns `ECHILD`.

Use this shape in `has_exited` before `waitpid_retry_on_eintr(pid_, &status, WNOHANG)`:

```cpp
        int status = 0;
        if (take_reaped_posix_child(pid_, &status)) {
            reaped_ = true;
            record_exit_status(status, &exit_code_);
            *exit_code = exit_code_;
            return true;
        }
```

- [ ] **Step 4: Install the reaper in server and host tests.**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, include `posix_child_reaper.h` under `#ifndef _WIN32` and call before `runtime.start_accept_loop()`:

```cpp
#ifndef _WIN32
    install_posix_child_reaper();
#endif
```

In `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`, include `posix_child_reaper.h` under `#ifndef _WIN32` and call `install_posix_child_reaper();` as the first statement in `main()` before resolving the default shell or constructing `SessionStore`.

- [ ] **Step 5: Add source inventory.**

In `crates/remote-exec-daemon-cpp/mk/sources.mk`, add:

```make
POSIX_CHILD_REAPER_SRCS = $(SOURCE_PREFIX)src/posix_child_reaper.cpp
```

Then include `$(POSIX_CHILD_REAPER_SRCS)` in `POSIX_SRCS`, `HOST_SERVER_STREAMING_SRCS`, `HOST_SESSION_STORE_SRCS`, `HOST_SERVER_RUNTIME_SRCS`, and `HOST_SERVER_ROUTES_SRCS`, because those groups link `process_session_posix.cpp`.

- [ ] **Step 6: Add Linux zombie regression coverage.**

In `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`, add these helpers under `#ifndef _WIN32`:

```cpp
#ifdef __linux__
static unsigned long zombie_children_of_current_process() {
    unsigned long zombies = 0UL;
    const fs::path proc("/proc");
    for (fs::directory_iterator it(proc), end; it != end; ++it) {
        const std::string name = it->path().filename().string();
        if (name.empty() || name.find_first_not_of("0123456789") != std::string::npos) {
            continue;
        }
        std::ifstream status((it->path() / "status").string().c_str());
        std::string line;
        bool zombie = false;
        long ppid = -1;
        while (std::getline(status, line)) {
            if (line.find("State:") == 0 && line.find("Z") != std::string::npos) {
                zombie = true;
            } else if (line.find("PPid:") == 0) {
                ppid = std::strtol(line.substr(5).c_str(), NULL, 10);
            }
        }
        if (zombie && ppid == static_cast<long>(getpid())) {
            ++zombies;
        }
    }
    return zombies;
}
#endif

static void assert_posix_sigchld_reaper_reaps_exited_session_children(
    const fs::path& root,
    const std::string& shell
) {
#ifdef __linux__
    const unsigned long baseline_zombies = zombie_children_of_current_process();
    SessionStore zombie_store;
    const YieldTimeConfig fast_yield = fast_yield_time_config();
    for (int index = 0; index < 5; ++index) {
        const Json running = start_test_command(
            zombie_store,
            "printf ready; (sleep 5 >&1) & sleep 0.2; exit 0",
            root.string(),
            shell,
            false,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            fast_yield,
            64UL
        );
        assert(running.at("running").get<bool>());
    }

    for (int attempt = 0; attempt < 40; ++attempt) {
        if (zombie_children_of_current_process() <= baseline_zombies) {
            return;
        }
        platform::sleep_ms(25UL);
    }
    assert(zombie_children_of_current_process() <= baseline_zombies);
#else
    (void)root;
    (void)shell;
#endif
}
```

Call `assert_posix_sigchld_reaper_reaps_exited_session_children(root, shell);` from `main()` after `assert_posix_exec_uses_parent_built_environment_and_path`.

- [ ] **Step 7: Run focused C++ verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
Expected: session-store tests pass, including the Linux zombie regression when `/proc` is available.

- [ ] **Step 8: Run broad C++ POSIX verification.**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: all POSIX C++ host tests and the POSIX daemon build pass.

- [ ] **Step 9: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/posix_child_reaper.h crates/remote-exec-daemon-cpp/src/posix_child_reaper.cpp crates/remote-exec-daemon-cpp/src/process_session_posix.cpp crates/remote-exec-daemon-cpp/src/server.cpp crates/remote-exec-daemon-cpp/mk/sources.mk crates/remote-exec-daemon-cpp/tests/test_session_store.cpp
git commit -m "fix: reap cpp posix session children"
```

### Task 9: Final D2 Quality Gate

**Files:**
- Verify only unless the commands expose formatting or lint issues.
- Modify only files already touched in Tasks 2-8 if formatting or lint fixes are required.

**Testing approach:** existing tests + full workspace quality gate
Reason: D2 changes touch public schemas, broker forwarding, Rust daemon behavior, C++ daemon behavior, and docs. The final gate must cover both focused D2 surfaces and workspace-wide lint/format expectations.

- [ ] **Step 1: Run focused Rust verification.**

Run:

```bash
cargo test -p remote-exec-proto
cargo test -p remote-exec-host
cargo test -p remote-exec-daemon --test exec_rpc
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-broker --test mcp_cli
```

Expected: all commands pass.

- [ ] **Step 2: Run focused C++ verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-routes
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: all commands pass.

- [ ] **Step 3: Run full Rust quality gate.**

Run:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all commands pass.

- [ ] **Step 4: Commit any verification fixes.**

If formatting or lint fixes were needed, commit only those fixes:

```bash
git add crates README.md skills/using-remote-exec-mcp/SKILL.md
git commit -m "chore: satisfy phase d2 quality gate"
```

If Step 1-3 pass without producing changes, do not create an empty commit.
