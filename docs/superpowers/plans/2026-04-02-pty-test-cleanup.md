# PTY Test Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the `stty` dependency from daemon PTY exec tests while keeping direct coverage for empty-poll PTY truncation and deterministic PTY stdin round-trip behavior.

**Architecture:** Keep all changes inside the daemon exec RPC test file. Replace the current `stty`-based PTY truncation test with a shell-builtin-only empty-poll test whose timing is chosen to land output in the `exec/write` response, then add a separate PTY round-trip test that checks for a stable output marker instead of exact raw PTY output.

**Tech Stack:** Rust 2024, Tokio async tests, daemon RPC integration tests, `cargo test`

---

## File Map

- Modify: `crates/remote-exec-daemon/tests/exec_rpc.rs`
  - This file owns daemon-level RPC integration coverage for `exec/start` and `exec/write`.
  - Replace the current `stty`-based PTY truncation test with a shell-builtin-only empty-poll PTY test.
  - Add a second PTY round-trip test that proves `write_stdin(chars="...")` reaches the child process without assuming echo behavior.
- Reference only: `crates/remote-exec-daemon/src/exec/mod.rs`
  - `exec_start` clamps `yield_time_ms` to a minimum of `250ms`.
  - `exec_write(chars="")` clamps empty polls to a minimum of `5000ms`.
  - The new polling test timing must account for those clamps so output appears in the poll response rather than the initial start response.

### Task 1: Replace The `stty`-Based PTY Truncation Test

**Files:**
- Modify: `crates/remote-exec-daemon/tests/exec_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc exec_output_write_truncates_to_max_output_tokens -- --exact --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc exec_empty_poll_truncates_pty_output_to_max_output_tokens -- --exact --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** `characterization/integration test`
Reason: this task only changes daemon RPC test coverage. We are refining an integration test to remove an OS-sensitive dependency while preserving the public daemon behavior being asserted.

- [ ] **Step 1: Capture the current PTY truncation test shape before editing**

```bash
cargo test -p remote-exec-daemon --test exec_rpc exec_output_write_truncates_to_max_output_tokens -- --exact --nocapture
```

Expected: PASS on the current tree, and the test still depends on `stty -echo` in `crates/remote-exec-daemon/tests/exec_rpc.rs`.

- [ ] **Step 2: Replace the old test with a shell-builtin-only empty-poll PTY truncation test**

```rust
#[tokio::test]
async fn exec_empty_poll_truncates_pty_output_to_max_output_tokens() {
    let fixture = support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "sleep 0.4; printf 'one two three four five six'; sleep 30".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: Some(3),
                login: Some(false),
            },
        )
        .await;

    assert!(started.running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started.daemon_session_id.expect("live session"),
                chars: "".to_string(),
                yield_time_ms: Some(5_000),
                max_output_tokens: Some(3),
            },
        )
        .await;

    assert!(response.running);
    assert_eq!(response.original_token_count, Some(6));
    assert_eq!(response.output, "one two three");
}
```

- [ ] **Step 3: Run the new empty-poll PTY truncation test**

```bash
cargo test -p remote-exec-daemon --test exec_rpc exec_empty_poll_truncates_pty_output_to_max_output_tokens -- --exact --nocapture
```

Expected: PASS, with no dependency on `stty` or terminal echo configuration.

- [ ] **Step 4: Run the full daemon exec RPC suite after the replacement**

```bash
cargo test -p remote-exec-daemon --test exec_rpc
```

Expected: PASS, confirming the renamed PTY truncation test still fits with the surrounding daemon exec coverage.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/tests/exec_rpc.rs
git commit -m "test: replace stty PTY truncation coverage"
```

### Task 2: Add Deterministic PTY Stdin Round-Trip Coverage

**Files:**
- Modify: `crates/remote-exec-daemon/tests/exec_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc exec_write_round_trips_pty_input_without_echo_assumptions -- --exact --nocapture`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** `characterization/integration test`
Reason: the daemon already supports PTY stdin writes. This task adds direct integration coverage for that behavior without relying on fragile assumptions about echoed PTY bytes.

- [ ] **Step 1: Add a PTY stdin round-trip test that asserts on a stable output marker**

```rust
#[tokio::test]
async fn exec_write_round_trips_pty_input_without_echo_assumptions() {
    let fixture = support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "IFS= read -r line; printf '__RESULT__:%s:__END__' \"$line\"".to_string(),
                workdir: None,
                shell: Some(TEST_SHELL.to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: None,
                login: Some(false),
            },
        )
        .await;

    assert!(started.running);

    let response = fixture
        .rpc::<ExecWriteRequest, ExecResponse>(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: started.daemon_session_id.expect("live session"),
                chars: "ping pong\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: None,
            },
        )
        .await;

    assert_eq!(response.exit_code, Some(0));
    assert!(response.output.contains("__RESULT__:ping pong:__END__"));
}
```

- [ ] **Step 2: Run the new PTY round-trip test by itself**

```bash
cargo test -p remote-exec-daemon --test exec_rpc exec_write_round_trips_pty_input_without_echo_assumptions -- --exact --nocapture
```

Expected: PASS, with output containing the stable marker regardless of whether the PTY echoes the input line.

- [ ] **Step 3: Re-run the full daemon exec RPC suite with both PTY tests present**

```bash
cargo test -p remote-exec-daemon --test exec_rpc
```

Expected: PASS, confirming the new round-trip test coexists cleanly with the rest of the daemon exec coverage.

- [ ] **Step 4: Run adjacent daemon smoke suites for quick regression coverage**

```bash
cargo test -p remote-exec-daemon --test health
cargo test -p remote-exec-daemon --test image_rpc
```

Expected: PASS, showing the test-only changes did not disturb nearby daemon suites.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/tests/exec_rpc.rs
git commit -m "test: add PTY write round-trip coverage"
```

## Spec Coverage Check

- Remove the `stty` dependency:
  - Covered by Task 1, Step 2.
- Keep direct `write_stdin(chars="")` PTY polling coverage:
  - Covered by Task 1, Step 2 and Step 3.
- Add deterministic PTY stdin round-trip coverage:
  - Covered by Task 2, Step 1 and Step 2.
- Avoid assertions that depend on echo behavior:
  - Covered by Task 2, Step 1 through `contains(...)` against a stable marker rather than full-output equality.

## Self-Review Notes

- No placeholders remain.
- The plan stays inside `crates/remote-exec-daemon/tests/exec_rpc.rs`, matching the approved spec scope.
- The polling task intentionally uses `sleep 0.4` rather than `sleep 0.1` so output lands after the `exec_start` minimum `250ms` window and before the `exec_write(chars="")` empty-poll window completes.
