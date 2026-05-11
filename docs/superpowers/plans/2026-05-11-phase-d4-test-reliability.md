# Phase D4 Test Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve only Phase D4 test-reliability risks from `docs/CODE_AUDIT_ROUND3.md`, without planning or implementing D3 observability or additional product behavior.

**Architecture:** Treat the audit as review input against the current tree, not as the live contract. D4 hardens test fixtures and CI coverage so broker/daemon regressions fail deterministically: stubs should mirror real daemon ID and patch behavior, spawned test tasks should surface panics, readiness loops should report named resources under bounded timeouts, port allocation should use test-only listener/bound-address APIs instead of dropped `:0` sockets, and port-forward tests should wait on observable conditions instead of fixed sleeps. Keep all changes in test support, test files, CI, and documentation except for build-only no-default-feature fallout if the added CI commands expose it.

**Tech Stack:** Rust 2024 workspace with Tokio/Axum/RMCP integration tests; standalone C++11 daemon test binaries with GNU make and optional Wine execution; GitHub Actions CI matrix; existing broker fixture modules under `crates/remote-exec-broker/tests/support`.

---

## Scope

Included from round-3 Phase D4:

- `#26`: Stub daemon hard-codes legacy daemon instance/session IDs and turns unknown exec-write sessions into handler panics.
- `#27`: Ephemeral port allocation TOCTOU across broker test fixtures and real C++ daemon integration tests.
- `#28`: Stub daemon port-tunnel workdirs leak under the system temp directory.
- `#29`: Discarded `tokio::spawn` handles hide panics in stub and spawner tasks.
- `#30`: Sleep-as-synchronization in Rust and C++ port-forward tests.
- `#31`: UDP negative assertions use too-tight 100 ms windows without paired positive proof.
- `#32`: Windows XP test binaries are built in CI but not executed when Wine is available.
- `#33`: No-default-features CI misses `remote-exec-host`.
- `#34`: Readiness loops lack explicit outer timeout messages naming the resource.
- `#35`: Stub `patch_apply` validates only the patch header, not the real parser footer.
- `#36`: C++ daemon Windows runtime coverage is absent from CI.

Explicitly excluded from this plan: D3 observability and operator-experience items, public tool/operator APIs, request correlation IDs, metrics, logging-level changes, exit-code taxonomy, and new C++ daemon product behavior. D4 may add hidden test-support APIs, build-system targets, and test-only startup hooks whose sole purpose is deterministic fixture binding and native Windows test execution. For `#36`, D4 must execute broker-to-C++ daemon runtime forwarding coverage on a native `windows-latest` runner against a host-native Windows C++ daemon process.

Current-state notes:

- Some D4 items already have partial mitigations. Plain-HTTP stub daemons are spawned with pre-bound Tokio listeners, but TLS stub daemon, Rust daemon fixture backend, broker streamable-HTTP child, real C++ daemon, and fixed remote-listener release tests still use dropped `:0` addresses.
- `multi_target` fixtures already store the main Rust daemon task handle, but their proxy accept tasks still spawn detached per-connection workers.
- `mcp_forward_ports_cpp.rs` has a file-level Unix cfg and its proxy also detaches per-connection workers; both must be addressed before #36 can run on Windows.
- The C++ GNU make path has POSIX-native and Windows-XP cross-build targets today, but no host-native Windows daemon target for the Rust broker integration fixture.
- The C++ POSIX child-reaper test added in D2 currently uses a short sleep to let `SIGCHLD` delivery/reaping occur. D4 should replace that with bounded polling.
- The XP Wine targets already exist in `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`: `test-wine-session-store` and `test-wine-transfer`. CI should install Wine on Linux and run them conditionally after `check-windows-xp`.

## File Structure

- `docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md`: this D4-only implementation plan.
- `crates/remote-exec-daemon/Cargo.toml`: add a `test-support` feature used by broker integration tests.
- `crates/remote-exec-daemon/src/test_support.rs`: expose listener-taking daemon and TLS/HTTP app servers for integration tests.
- `crates/remote-exec-daemon/src/lib.rs`: publish the `test_support` module behind the `test-support` feature and share the run path with listener-taking test support.
- `crates/remote-exec-daemon/src/server.rs`: add an internal listener-taking server path that still joins daemon background tasks.
- `crates/remote-exec-daemon/src/tls.rs`, `crates/remote-exec-daemon/src/tls_enabled.rs`, `crates/remote-exec-daemon/src/tls_disabled.rs`: split internal bind-and-serve from listener-taking serve helpers.
- `crates/remote-exec-broker/Cargo.toml`: enable the daemon `test-support` feature for broker integration tests.
- `crates/remote-exec-broker/src/mcp_server.rs`: add hidden test-only bound-address file reporting for streamable HTTP listener startup.
- `crates/remote-exec-broker/tests/support/stub_daemon.rs`: dynamic daemon/session IDs, typed unknown-session responses, stub patch footer parity, tracked stub tasks, owned port-tunnel tempdir, and readiness timeouts.
- `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`: use generated daemon session ID and typed validation instead of `assert_eq!`.
- `crates/remote-exec-broker/tests/support/fixture.rs`: expose stub daemon session ID helper and add targeted regression assertions where fixture-level helpers fit.
- `crates/remote-exec-broker/src/tools/exec_format.rs`: replace the remaining hard-coded unit-test daemon instance ID fixture with a generated real-format ID.
- `crates/remote-exec-broker/tests/support/spawners.rs`: track spawned stub task handles, add outer timeouts to readiness loops, wait for broker child bound-address files, and keep dead-target allocation explicit.
- `crates/remote-exec-broker/tests/mcp_exec/session.rs`: add public-surface regression that a stale daemon session produces typed unknown-session behavior rather than a 500 from the stub.
- `crates/remote-exec-broker/tests/mcp_assets.rs`: add malformed patch footer regression through broker-to-stub path.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs`: replace fixed sleeps with status/counter helpers and widen UDP negative windows with positive counterparts.
- `crates/remote-exec-broker/tests/multi_target.rs`: replace fixed sleeps and dropped-address listener-open tests with condition polling and returned-endpoint assertions.
- `crates/remote-exec-broker/tests/multi_target/support.rs`: use listener-taking daemon test APIs, wait for broker child bound-address files, readiness outer timeouts, proxy task tracking, and helper changes needed by `multi_target.rs`.
- `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`: remove the Unix-only file cfg, start C++ daemon and broker child fixtures with port `0`, wait for bound-address files, remove dropped-address helpers, track proxy tasks, add readiness outer timeouts, replace fixed sleeps, and run a Windows-safe subset natively on `windows-latest`.
- `crates/remote-exec-daemon-cpp/GNUmakefile`: include the host-native Windows GNU make target file.
- `crates/remote-exec-daemon-cpp/mk/windows-native.mk`: add host-native Windows MinGW build rules for `build/remote-exec-daemon-cpp.exe`.
- `crates/remote-exec-daemon-cpp/include/config.h`: add hidden `test_bound_addr_file` fixture field.
- `crates/remote-exec-daemon-cpp/src/config.cpp`: allow `listen_port = 0` only when `test_bound_addr_file` is present.
- `crates/remote-exec-daemon-cpp/src/server.cpp`: write the actual bound address to `test_bound_addr_file` after the listener is live.
- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`: replace sleep-based child-reaper test synchronization with bounded polling.
- `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`: replace long fixed sleep in socket-buffer pressure test with condition-driven blocking.
- `.github/workflows/ci.yml`: add host no-default-features test/clippy commands, conditional Wine execution for XP test binaries, and a native Windows broker-to-C++ daemon runtime test job/step.
- `README.md`: update focused no-default-features commands and CI coverage notes.
- `crates/remote-exec-daemon-cpp/README.md`: document what the XP Wine targets execute and how native Windows C++ daemon runtime coverage is executed.

---

### Task 1: Save The Phase D4 Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md`
- Test/Verify: `git status --short docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked plan artifact only. The repo already tracks many files under `docs/superpowers/plans`, so the D4 plan follows that convention.

- [ ] **Step 1: Verify this plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md`
Expected: command exits successfully.

- [ ] **Step 2: Review the plan heading and scope.**

Run: `sed -n '1,90p' docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md`
Expected: output names Phase D4 only, includes the required agentic-worker header, includes all D4 audit items, and explicitly excludes D3.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md
git commit -m "docs: plan phase d4 test reliability"
```

### Task 2: Make Stub Exec IDs Realistic And Typed

**Finding:** D4 `#26`

**Files:**
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`
- Modify: `crates/remote-exec-broker/tests/support/fixture.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec_format.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec/session.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --lib tools::exec_format::tests::format_command_text_includes_original_token_count_when_present`
  - `cargo test -p remote-exec-broker --test mcp_exec write_stdin`

**Testing approach:** TDD
Reason: The defect has a clear broker-visible seam: the stub should return generated `inst_...`/`sess_...` IDs and stale daemon-session writes should become typed `unknown_session` responses, not handler panics/500s.

- [ ] **Step 1: Add a fixture helper exposing the daemon session ID returned by the stub.**

In `crates/remote-exec-broker/tests/support/fixture.rs`, add this method inside the second `impl BrokerFixture` block:

```rust
    pub async fn stub_daemon_session_id(&self) -> String {
        self.stub_state.daemon_session_id.lock().await.clone()
    }
```

- [ ] **Step 2: Add a failing public regression for realistic generated daemon session IDs.**

In `crates/remote-exec-broker/tests/mcp_exec/session.rs`, add:

```rust
#[tokio::test]
async fn write_stdin_uses_generated_daemon_session_id_from_stub() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let daemon_session_id = fixture.stub_daemon_session_id().await;
    assert!(
        daemon_session_id.starts_with("sess_"),
        "stub daemon session id should look like a real daemon session id: {daemon_session_id}"
    );

    let started = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 10
            }),
        )
        .await;
    let session_id = started.structured_content["session_id"]
        .as_str()
        .expect("running session");

    fixture
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-a",
                "chars": ""
            }),
        )
        .await;

    let forwarded = fixture
        .last_exec_write_request()
        .await
        .expect("write request");
    assert_eq!(forwarded.daemon_session_id, daemon_session_id);
}
```

- [ ] **Step 3: Add a failing typed stale-daemon-session regression.**

In the same file, add:

```rust
#[tokio::test]
async fn write_stdin_wraps_stub_stale_daemon_session_as_unknown_process_id() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let started = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 10
            }),
        )
        .await;
    let public_session_id = started.structured_content["session_id"]
        .as_str()
        .expect("running session")
        .to_string();

    fixture
        .set_stub_daemon_session_id("sess_replaced_by_test")
        .await;

    let error = fixture
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": public_session_id,
                "target": "builder-a",
                "chars": ""
            }),
        )
        .await;

    assert_eq!(
        error,
        format!("write_stdin failed: Unknown process id {public_session_id}")
    );
}
```

Add this helper beside `set_stub_daemon_instance_id` in `fixture.rs`:

```rust
    pub async fn set_stub_daemon_session_id(&self, daemon_session_id: &str) {
        *self.stub_state.daemon_session_id.lock().await = daemon_session_id.to_string();
    }
```

- [ ] **Step 4: Run the focused test and confirm it fails.**

Run: `cargo test -p remote-exec-broker --test mcp_exec write_stdin -- --nocapture`
Expected: the generated-ID assertion fails because the stub still returns `"daemon-session-1"`, and/or the stale-session test reports a 500-style error instead of a typed unknown-process error.

- [ ] **Step 5: Add generated IDs to the stub state.**

In `crates/remote-exec-broker/tests/support/stub_daemon.rs`, update `StubDaemonState`:

```rust
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    pub(super) daemon_session_id: Arc<Mutex<String>>,
```

Update `stub_daemon_state`:

```rust
        daemon_instance_id: Arc::new(Mutex::new(
            remote_exec_host::ids::new_instance_id().into_string(),
        )),
        daemon_session_id: Arc::new(Mutex::new(
            remote_exec_host::ids::new_exec_session_id().into_string(),
        )),
```

Update `health` to take state and return the same instance ID as `/v1/target-info`:

```rust
async fn health(State(state): State<StubDaemonState>) -> Json<HealthCheckResponse> {
    Json(HealthCheckResponse {
        status: "ok".to_string(),
        daemon_version: "0.1.0".to_string(),
        daemon_instance_id: state.daemon_instance_id.lock().await.clone(),
    })
}
```

Update the router health route remains unchanged:

```rust
.route("/v1/health", post(health))
```

Update the `target_config_fragment_renders_insecure_http_target` test fixture construction in `multi_target/support.rs` only when the compiler points to missing fields; for this stub state task, construction sites are in `stub_daemon_state` only.

- [ ] **Step 6: Update exec start/write to use typed daemon session validation.**

In `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`, update `exec_start` before building the response:

```rust
    let daemon_session_id = state.daemon_session_id.lock().await.clone();
```

Use it in the running response:

```rust
                daemon_session_id,
```

In `exec_write`, replace `assert_eq!(req.daemon_session_id, "daemon-session-1");` with:

```rust
    let expected_daemon_session_id = state.daemon_session_id.lock().await.clone();
    if req.daemon_session_id != expected_daemon_session_id {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "unknown_session".to_string(),
                message: "Unknown daemon session".to_string(),
            }),
        ));
    }
```

- [ ] **Step 7: Run focused verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_exec write_stdin -- --nocapture
```

Expected: all `write_stdin`-filtered tests pass.

- [ ] **Step 8: Replace the remaining hard-coded exec-format fixture instance ID.**

In `crates/remote-exec-broker/src/tools/exec_format.rs`, update `completed_response` so the unit-test `ExecOutputResponse` uses a generated daemon instance ID:

```rust
    fn completed_response() -> ExecResponse {
        ExecResponse::Completed(ExecCompletedResponse {
            output: ExecOutputResponse {
                daemon_instance_id: remote_exec_host::ids::new_instance_id().into_string(),
                running: false,
                chunk_id: Some("abc123".to_string()),
                wall_time_seconds: 0.25,
                exit_code: Some(0),
                original_token_count: Some(6),
                output: "one two three".to_string(),
                warnings: Vec::new(),
            },
        })
    }
```

- [ ] **Step 9: Run the exec-format unit verification.**

Run:

```bash
cargo test -p remote-exec-broker --lib tools::exec_format::tests::format_command_text_includes_original_token_count_when_present
```

Expected: the unit test passes and no `"daemon-instance-1"` literal remains in `exec_format.rs`.

- [ ] **Step 10: Confirm all legacy stub ID literals are gone from the planned surfaces.**

Run:

```bash
! rg -n '"daemon-instance-1"|"daemon-session-1"' crates/remote-exec-broker/tests/support crates/remote-exec-broker/tests/mcp_exec crates/remote-exec-broker/src/tools/exec_format.rs
```

Expected: command exits successfully with no matches.

- [ ] **Step 11: Commit.**

```bash
git add crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/support/stub_daemon_exec.rs crates/remote-exec-broker/tests/support/fixture.rs crates/remote-exec-broker/src/tools/exec_format.rs crates/remote-exec-broker/tests/mcp_exec/session.rs
git commit -m "test: use realistic stub exec ids"
```

### Task 3: Mirror Real Patch Footer Validation In Stub

**Finding:** D4 `#35`

**Files:**
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets malformed_patch_footer`

**Testing approach:** TDD
Reason: The broker test stub currently accepts malformed patches the real parser rejects. A broker-facing integration test can prove stub parity without exposing host internals.

- [ ] **Step 1: Add a failing malformed-footer broker test.**

In `crates/remote-exec-broker/tests/mcp_assets.rs`, add:

```rust
#[tokio::test]
async fn malformed_patch_footer_is_rejected_by_stub_daemon() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: missing-footer.txt\n+hello\n"
            }),
        )
        .await;

    assert!(
        error.contains("invalid patch footer"),
        "unexpected malformed patch footer error: {error}"
    );
}
```

- [ ] **Step 2: Run the test and confirm it fails.**

Run: `cargo test -p remote-exec-broker --test mcp_assets malformed_patch_footer -- --nocapture`
Expected: the test fails because the stub accepts the patch and the broker call succeeds.

- [ ] **Step 3: Add header/footer parity validation to the stub.**

In `crates/remote-exec-broker/tests/support/stub_daemon.rs`, add:

```rust
fn trim_horizontal(value: &str) -> &str {
    value.trim_matches([' ', '\t'])
}
```

Replace the current `patch_apply` validation block with:

```rust
    let lines = req.patch.lines().collect::<Vec<_>>();
    if lines.first().copied().map(trim_horizontal) != Some("*** Begin Patch") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "patch_failed".to_string(),
                message: "invalid patch header".to_string(),
            }),
        ));
    }
    if lines.last().copied().map(trim_horizontal) != Some("*** End Patch") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "patch_failed".to_string(),
                message: "invalid patch footer".to_string(),
            }),
        ));
    }
```

- [ ] **Step 4: Run focused verification.**

Run: `cargo test -p remote-exec-broker --test mcp_assets malformed_patch_footer -- --nocapture`
Expected: test passes.

- [ ] **Step 5: Run broader asset tests.**

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: all asset tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/mcp_assets.rs
git commit -m "test: mirror patch footer validation in stub"
```

### Task 4: Bound Readiness Loops With Named Timeouts

**Finding:** D4 `#34`

**Files:**
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_assets list_targets_includes_enabled_local_target`
  - `cargo test -p remote-exec-broker --test multi_target support::tests::target_config_fragment_renders_insecure_http_target`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp list_targets_reports_port_forward_protocol_version_for_real_cpp_daemon`

**Testing approach:** existing tests + targeted verification
Reason: This is fixture reliability plumbing. The behavior is clearer failure messages and bounded waits, best verified by compiling and running representative tests that exercise each readiness helper.

- [ ] **Step 1: Add a generic polling helper to `stub_daemon.rs`.**

In `crates/remote-exec-broker/tests/support/stub_daemon.rs`, add near the readiness functions:

```rust
const STUB_READY_TIMEOUT: Duration = Duration::from_secs(5);
const STUB_READY_POLL: Duration = Duration::from_millis(50);
```

Update `wait_until_ready` body to wrap the loop:

```rust
    tokio::time::timeout(STUB_READY_TIMEOUT, async {
        loop {
            if client
                .post(format!("https://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(STUB_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("TLS stub daemon at https://{addr} did not become ready within {STUB_READY_TIMEOUT:?}"));
```

Update `wait_until_ready_http` body to use the same timeout and polling constants:

```rust
    tokio::time::timeout(STUB_READY_TIMEOUT, async {
        loop {
            if client
                .post(format!("http://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(STUB_READY_POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("plain HTTP stub daemon at http://{addr} did not become ready within {STUB_READY_TIMEOUT:?}"));
```

- [ ] **Step 2: Add named timeout loops to `spawners.rs`.**

In `crates/remote-exec-broker/tests/support/spawners.rs`, add near the readiness functions:

```rust
const TEST_HTTP_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const TEST_HTTP_READY_POLL: std::time::Duration = std::time::Duration::from_millis(50);
```

Rewrite `wait_until_ready_http` with `tokio::time::timeout(TEST_HTTP_READY_TIMEOUT, async { loop { ... } })` and this panic:

```rust
panic!("plain HTTP stub daemon at http://{addr} did not become ready within {TEST_HTTP_READY_TIMEOUT:?}")
```

Rewrite `wait_until_ready_mcp_http` the same way with:

```rust
panic!("streamable HTTP broker at {url} did not become ready within {TEST_HTTP_READY_TIMEOUT:?}")
```

- [ ] **Step 3: Add named timeout loops to `multi_target/support.rs`.**

In `crates/remote-exec-broker/tests/multi_target/support.rs`, add:

```rust
const MULTI_TARGET_READY_TIMEOUT: Duration = Duration::from_secs(20);
const MULTI_TARGET_READY_POLL: Duration = Duration::from_millis(50);
```

Rewrite `wait_until_ready_http` and `wait_until_ready_mcp_http` to use `tokio::time::timeout` around their loops. Use messages that name the resource:

```rust
"daemon HTTP endpoint at http://{addr} did not become ready within {MULTI_TARGET_READY_TIMEOUT:?}"
"streamable HTTP broker at {url} did not become ready within {MULTI_TARGET_READY_TIMEOUT:?}"
```

For `wait_for_listener_release` and `wait_for_daemon_listener_rebind`, keep the existing caller-provided timeout but make the panic include the endpoint and timeout:

```rust
panic!("listener {addr} was not released within {timeout:?}");
panic!("daemon listener on {endpoint} was not released within {timeout:?}");
```

- [ ] **Step 4: Add named timeout loops to `mcp_forward_ports_cpp.rs`.**

Add:

```rust
const CPP_READY_TIMEOUT: Duration = Duration::from_secs(20);
const CPP_READY_POLL: Duration = Duration::from_millis(50);
```

Rewrite `wait_until_ready_http` and `wait_until_ready_mcp_http` with outer `tokio::time::timeout`, using messages:

```rust
"real C++ daemon at http://{addr} did not become ready within {CPP_READY_TIMEOUT:?}"
"broker MCP HTTP endpoint at {url} did not become ready within {CPP_READY_TIMEOUT:?}"
```

- [ ] **Step 5: Run targeted verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_assets list_targets_includes_enabled_local_target
cargo test -p remote-exec-broker --test multi_target support::tests::target_config_fragment_renders_insecure_http_target
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp list_targets_reports_port_forward_protocol_version_for_real_cpp_daemon
```

Expected: all commands pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/support/spawners.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "test: bound readiness waits with named timeouts"
```

### Task 5: Remove Stub Tempdir Leaks And Track Stub Tasks

**Findings:** D4 `#28`, D4 `#29`

**Files:**
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/support/fixture.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_connect_side_reconnect_retries_transient_open_failures`
  - `cargo test -p remote-exec-broker --test mcp_exec write_stdin_routes_by_public_session_id_and_preserves_original_command_metadata`
  - `cargo test -p remote-exec-broker --test multi_target forward_ports_reconnect_after_connect_side_tunnel_drop_and_accept_new_tcp_connections`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp cpp_forward_ports_reconnect_after_connect_tunnel_drop`

**Testing approach:** existing tests + targeted verification
Reason: This changes test-fixture ownership and panic surfacing. Representative exec, Rust proxy, C++ proxy, and port-tunnel tests exercise the tracked tasks and state lifetime without needing new product tests.

- [ ] **Step 1: Add task tracking helpers and tempdir ownership to `StubDaemonState`.**

In `crates/remote-exec-broker/tests/support/stub_daemon.rs`, add imports:

```rust
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use tokio::task::JoinHandle;
```

Extend `StubDaemonState`:

```rust
    _port_tunnel_tempdir: Arc<tempfile::TempDir>,
    background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
```

In `stub_daemon_state`, create the tempdir before the struct literal:

```rust
    let port_tunnel_tempdir = Arc::new(tempfile::tempdir().expect("stub port tunnel tempdir"));
```

Set fields:

```rust
        _port_tunnel_tempdir: port_tunnel_tempdir.clone(),
        port_tunnel_state: build_stub_port_tunnel_state(target, port_tunnel_tempdir.path()),
        background_tasks: Arc::new(Mutex::new(Vec::new())),
```

Change `build_stub_port_tunnel_state` signature and workdir:

```rust
fn build_stub_port_tunnel_state(
    target: &str,
    tempdir: &std::path::Path,
) -> Arc<remote_exec_host::HostRuntimeState> {
    let workdir = tempdir.join("port-tunnel-workdir");
    std::fs::create_dir_all(&workdir).unwrap();
```

Then add these helpers:

```rust
async fn spawn_stub_task(
    state: &StubDaemonState,
    name: &'static str,
    task: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
) {
    let handle = tokio::spawn(async move {
        match std::panic::AssertUnwindSafe(task).catch_unwind().await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => panic!("stub daemon background task `{name}` failed: {err:?}"),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    });
    state.background_tasks.lock().await.push(handle);
}

pub(crate) async fn assert_no_stub_task_panics(state: &StubDaemonState) {
    let finished = {
        let mut tasks = state.background_tasks.lock().await;
        let mut finished = Vec::new();
        let mut pending = Vec::with_capacity(tasks.len());
        for handle in tasks.drain(..) {
            if handle.is_finished() {
                finished.push(handle);
            } else {
                pending.push(handle);
            }
        }
        *tasks = pending;
        finished
    };

    for handle in finished {
        handle.await.expect("stub daemon background task panicked");
    }
}
```

- [ ] **Step 2: Track spawned stub server and tunnel tasks.**

Replace detached `tokio::spawn` calls in `spawn_named_daemon_on_addr`, `spawn_named_plain_http_daemon_on_listener`, and `port_tunnel` with `spawn_stub_task(&state, "...", async move { ... }).await;`.

For the TLS server:

```rust
    let task_state = state.clone();
    spawn_stub_task(&state, "tls-server", async move {
        remote_exec_daemon::tls::serve_tls(app, Arc::new(daemon_config)).await
    })
    .await;
    wait_until_ready(certs, addr).await;
    assert_no_stub_task_panics(&task_state).await;
```

For the plain HTTP server:

```rust
    let task_state = state.clone();
    spawn_stub_task(&state, "plain-http-server", async move {
        axum::serve(listener, app).await.map_err(Into::into)
    })
    .await;
    wait_until_ready_http(addr).await;
    assert_no_stub_task_panics(&task_state).await;
```

For the upgraded tunnel handler:

```rust
    spawn_stub_task(&state, "port-tunnel-upgrade", async move {
        let upgraded = on_upgrade.await?;
        handle_port_tunnel_upgrade(handler_state, TokioIo::new(upgraded)).await
    })
    .await;
```

For `handle_port_tunnel_upgrade`, replace the detached `serve_tunnel` spawn with:

```rust
state
    .port_tunnel_state
    .background_tasks
    .spawn("stub-inner-port-tunnel", async move {
        remote_exec_host::port_forward::serve_tunnel(tunnel_state, daemon_side).await
    })
    .await;
```

so it is tracked by the host runtime's existing background task tracker.

- [ ] **Step 3: Expose task-panic checks from `BrokerFixture`.**

In `crates/remote-exec-broker/tests/support/fixture.rs`, import and add:

```rust
    pub async fn assert_no_stub_task_panics(&self) {
        super::stub_daemon::assert_no_stub_task_panics(&self.stub_state).await;
    }
```

- [ ] **Step 4: Replace direct spawns in `spawners.rs` helper paths.**

In `crates/remote-exec-broker/tests/support/spawners.rs`, replace local `tokio::spawn(async move { serve(listener, app).await.unwrap(); });` blocks with calls to a new helper in `stub_daemon.rs`:

```rust
pub(crate) async fn spawn_plain_http_stub_on_listener(
    listener: tokio::net::TcpListener,
    state: StubDaemonState,
) {
    spawn_named_plain_http_daemon_on_listener(listener, state).await;
}
```

Use that helper in `spawn_broker_with_stub_daemon_http_auth`, `spawn_broker_with_stub_port_forward_version`, `spawn_broker_with_local_and_stub_port_forward_version_and_extra_config`, and `spawn_broker_with_plain_http_stub_daemon`.

- [ ] **Step 5: Track Rust multi-target proxy accept and connection tasks.**

In `crates/remote-exec-broker/tests/multi_target/support.rs`, extend `TunnelDropProxy` with a background task list:

```rust
    background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
```

Initialize it in `TunnelDropProxy::spawn`:

```rust
        let background_tasks = Arc::new(Mutex::new(Vec::new()));
        let background_tasks_accept = background_tasks.clone();
```

Inside the accept loop, replace the detached per-connection spawn with tracked handles:

```rust
                        let connection_handle = tokio::spawn(async move {
                            if let Err(err) = proxy_connection(stream, backend_addr, active_port_tunnels).await {
                                panic!("multi-target tunnel-drop proxy connection failed: {err}");
                            }
                        });
                        background_tasks_accept.lock().await.push(connection_handle);
```

Add this method:

```rust
    async fn assert_no_task_panics(&self) {
        let finished = {
            let mut tasks = self.background_tasks.lock().await;
            let mut finished = Vec::new();
            let mut pending = Vec::new();
            for handle in tasks.drain(..) {
                if handle.is_finished() {
                    finished.push(handle);
                } else {
                    pending.push(handle);
                }
            }
            *tasks = pending;
            finished
        };
        for handle in finished {
            handle.await.expect("multi-target tunnel-drop proxy task panicked");
        }
    }
```

Call it from `DaemonFixture::drop_port_tunnels` immediately after forwarding the drop to the proxy:

```rust
        self.proxy.drop_port_tunnels().await;
        self.proxy.assert_no_task_panics().await;
```

- [ ] **Step 6: Track C++ tunnel-drop proxy accept and connection tasks.**

In `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`, add the same `background_tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>` field to `TunnelDropProxy`.

In `TunnelDropProxy::spawn`, initialize it and wrap each `proxy_connection` spawn:

```rust
        let background_tasks = Arc::new(Mutex::new(Vec::new()));
        let background_tasks_accept = background_tasks.clone();
```

```rust
                        let connection_handle = tokio::spawn(async move {
                            if let Err(err) = proxy_connection(stream, daemon_addr, active_port_tunnels).await {
                                panic!("C++ tunnel-drop proxy connection failed: {err}");
                            }
                        });
                        background_tasks_accept.lock().await.push(connection_handle);
```

Add the same `assert_no_task_panics` helper and call it from `TunnelDropProxy::drop_port_tunnels` after sending all drop signals:

```rust
        self.assert_no_task_panics().await;
```

Keep `stop` aborting still-pending tasks during fixture teardown:

```rust
        if let Ok(mut tasks) = self.background_tasks.try_lock() {
            for handle in tasks.drain(..) {
                handle.abort();
            }
        }
```

- [ ] **Step 7: Confirm proxy task spawns are no longer detached.**

Run:

```bash
! rg -n "tokio::spawn\\(async move \\{\\s*let _ = proxy_connection" crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
```

Expected: command exits successfully with no matches.

- [ ] **Step 8: Run targeted verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_connect_side_reconnect_retries_transient_open_failures
cargo test -p remote-exec-broker --test mcp_exec write_stdin_routes_by_public_session_id_and_preserves_original_command_metadata
cargo test -p remote-exec-broker --test multi_target forward_ports_reconnect_after_connect_side_tunnel_drop_and_accept_new_tcp_connections
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp cpp_forward_ports_reconnect_after_connect_tunnel_drop
```

Expected: all commands pass.

- [ ] **Step 9: Commit.**

```bash
git add crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/support/spawners.rs crates/remote-exec-broker/tests/support/fixture.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "test: track stub daemon background tasks"
```

### Task 6: Add Rust Daemon Listener-Taking Test APIs

**Finding:** D4 `#27`

**Files:**
- Modify: `crates/remote-exec-daemon/Cargo.toml`
- Create: `crates/remote-exec-daemon/src/test_support.rs`
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Modify: `crates/remote-exec-daemon/src/tls.rs`
- Modify: `crates/remote-exec-daemon/src/tls_enabled.rs`
- Modify: `crates/remote-exec-daemon/src/tls_disabled.rs`
- Modify: `crates/remote-exec-broker/Cargo.toml`
- Test/Verify:
  - `cargo check -p remote-exec-daemon --features test-support --tests`
  - `cargo check -p remote-exec-broker --tests`

**Testing approach:** compile-level test-support API verification
Reason: This task adds hidden test APIs only. The next task uses them from broker integration fixtures; this task's proof is that the feature-gated daemon API compiles with and without broker tests.

- [ ] **Step 1: Add the daemon feature flag and broker dev-dependency feature.**

In `crates/remote-exec-daemon/Cargo.toml`, add:

```toml
test-support = []
```

Keep `default = ["tls", "winpty"]` unchanged.

In `crates/remote-exec-broker/Cargo.toml`, change the daemon dev-dependency to:

```toml
remote-exec-daemon = { path = "../remote-exec-daemon", features = ["test-support"] }
```

- [ ] **Step 2: Add listener-taking daemon run support.**

In `crates/remote-exec-daemon/src/lib.rs`, add:

```rust
#[cfg(feature = "test-support")]
pub mod test_support;
```

Then replace the body of `run_until` with a listener-binding wrapper:

```rust
pub async fn run_until<F>(config: DaemonConfig, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    tls::install_crypto_provider()?;
    let daemon_config = Arc::new(config);
    let listener = tls::bind_listener(daemon_config.listen)?;
    run_until_on_bound_listener(daemon_config, listener, shutdown).await
}
```

Add this internal helper below it:

```rust
pub(crate) async fn run_until_on_bound_listener<F>(
    daemon_config: Arc<DaemonConfig>,
    listener: tokio::net::TcpListener,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    tls::install_crypto_provider()?;
    let state = remote_exec_host::build_runtime_state(daemon_config.host_runtime_config())?;
    tracing::info!(
        target = %daemon_config.target,
        listen = %listener.local_addr().unwrap_or(daemon_config.listen),
        transport = ?daemon_config.transport,
        http_auth_enabled = daemon_config.http_auth.is_some(),
        default_workdir = %daemon_config.default_workdir.display(),
        default_shell = %state.default_shell,
        supports_pty = state.supports_pty,
        supports_transfer_compression = state.supports_transfer_compression,
        pty_mode = ?daemon_config.pty,
        daemon_instance_id = %state.daemon_instance_id,
        "starting daemon"
    );
    let shutdown_state = state.clone();
    let shutdown = async move {
        shutdown.await;
        shutdown_state.shutdown.cancel();
    };
    server::serve_with_shutdown_on_listener(state, daemon_config, listener, shutdown).await
}
```

- [ ] **Step 3: Add listener-taking server plumbing.**

In `crates/remote-exec-daemon/src/server.rs`, change `serve_with_shutdown` to bind through TLS and delegate:

```rust
pub async fn serve_with_shutdown<F>(
    state: AppState,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let listener = crate::tls::bind_listener(daemon_config.listen)?;
    serve_with_shutdown_on_listener(state, daemon_config, listener, shutdown).await
}
```

Add:

```rust
pub(crate) async fn serve_with_shutdown_on_listener<F>(
    state: AppState,
    daemon_config: Arc<DaemonConfig>,
    listener: tokio::net::TcpListener,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send,
{
    let state = Arc::new(state);
    let app = crate::http::routes::router(state.clone(), daemon_config.clone());
    let result =
        crate::tls::serve_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await;
    state.background_tasks.join_all().await;
    result
}
```

- [ ] **Step 4: Split TLS/HTTP serve into bind-and-serve and listener-taking helpers.**

In `crates/remote-exec-daemon/src/tls.rs`, make `bind_listener` crate-visible:

```rust
pub(crate) fn bind_listener(addr: std::net::SocketAddr) -> std::io::Result<TcpListener> {
```

Change `serve_http_with_shutdown` to delegate:

```rust
pub async fn serve_http_with_shutdown<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let listener = bind_listener(daemon_config.listen)?;
    serve_http_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
}
```

Add:

```rust
pub(crate) async fn serve_with_shutdown_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    match daemon_config.transport {
        DaemonTransport::Tls => {
            tls_impl::serve_tls_with_shutdown_on_listener(app, daemon_config, listener, shutdown)
                .await
        }
        DaemonTransport::Http => {
            serve_http_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
        }
    }
}
```

and:

```rust
pub(crate) async fn serve_http_with_shutdown_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let local_addr = listener.local_addr()?;
    tracing::info!(listen = %local_addr, "daemon http listener bound");
    let accept_stream: AcceptStream =
        Arc::new(|stream| Box::pin(async move { Ok(Some(Box::new(stream) as AcceptedStream)) }));
    serve_http1_connections(listener, app, shutdown, accept_stream, "http").await
}
```

In `crates/remote-exec-daemon/src/tls_enabled.rs`, change `serve_tls_with_shutdown` to delegate:

```rust
pub async fn serve_tls_with_shutdown<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let listener = super::bind_listener(daemon_config.listen)?;
    serve_tls_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
}
```

Add:

```rust
pub(crate) async fn serve_tls_with_shutdown_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: tokio::net::TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let local_addr = listener.local_addr()?;
    tracing::info!(listen = %local_addr, "daemon tls listener bound");
    let tls = TlsAcceptor::from(Arc::new(server_config(daemon_config.as_ref()).await?));
    let accept_stream: AcceptStream = Arc::new(move |stream| {
        let tls = tls.clone();
        Box::pin(async move {
            match tls.accept(stream).await {
                Ok(stream) => Ok(Some(Box::new(stream) as AcceptedStream)),
                Err(err) => {
                    tracing::warn!(?err, "tls accept failed");
                    Ok(None)
                }
            }
        })
    });
    serve_http1_connections(listener, app, shutdown, accept_stream, "tls").await
}
```

Move the existing TLS accept-stream body into this helper rather than duplicating it.

In `crates/remote-exec-daemon/src/tls_disabled.rs`, add:

```rust
pub(crate) async fn serve_tls_with_shutdown_on_listener<F>(
    _: Router,
    _: Arc<DaemonConfig>,
    _: tokio::net::TcpListener,
    _: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    anyhow::bail!(super::FEATURE_REQUIRED_MESSAGE);
}
```

- [ ] **Step 5: Expose only the hidden test-support API.**

Create `crates/remote-exec-daemon/src/test_support.rs`:

```rust
use std::future::Future;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

use crate::config::DaemonConfig;

pub async fn run_until_on_listener<F>(
    config: DaemonConfig,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    crate::run_until_on_bound_listener(Arc::new(config), listener, shutdown).await
}

pub async fn serve_tls_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    crate::tls::serve_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
}

pub async fn serve_http_on_listener<F>(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    listener: TcpListener,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    crate::tls::serve_http_with_shutdown_on_listener(app, daemon_config, listener, shutdown).await
}
```

- [ ] **Step 6: Run compile verification.**

Run:

```bash
cargo check -p remote-exec-daemon --features test-support --tests
cargo check -p remote-exec-broker --tests
```

Expected: both commands pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-daemon/Cargo.toml crates/remote-exec-daemon/src/test_support.rs crates/remote-exec-daemon/src/lib.rs crates/remote-exec-daemon/src/server.rs crates/remote-exec-daemon/src/tls.rs crates/remote-exec-daemon/src/tls_enabled.rs crates/remote-exec-daemon/src/tls_disabled.rs crates/remote-exec-broker/Cargo.toml
git commit -m "test: add daemon listener test support"
```

### Task 7: Use Listener-Taking APIs In Rust Fixtures

**Finding:** D4 `#27`

**Files:**
- Modify: `crates/remote-exec-broker/tests/support/certs.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target.rs`
- Test/Verify:
  - `! rg -n "allocate_addr\\(" crates/remote-exec-broker/tests/support/certs.rs crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/multi_target.rs`
  - `cargo test -p remote-exec-broker --test mcp_exec broker_rejects_unverified_target_if_it_returns_as_the_wrong_daemon -- --nocapture`
  - `cargo test -p remote-exec-broker --test multi_target forward_ports_release_remote_listeners_when_broker_stops -- --nocapture`

**Testing approach:** existing tests + static dropped-port scan
Reason: The changed behavior is fixture startup ownership. The scan proves the audited dropped-address helper is gone from Rust daemon startup paths, while the tests prove late-target and returned-endpoint release workflows still work.

- [ ] **Step 1: Remove the generic cert address allocator.**

Delete this helper from `crates/remote-exec-broker/tests/support/certs.rs`:

```rust
pub(crate) fn allocate_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}
```

In `crates/remote-exec-broker/tests/support/stub_daemon.rs`, remove:

```rust
use super::certs::allocate_addr;
```

- [ ] **Step 2: Spawn TLS stub daemons from a retained listener.**

In `spawn_daemon_with_platform`, replace the dropped address allocation with:

```rust
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind TLS stub daemon listener");
    let addr = listener.local_addr().expect("read TLS stub daemon addr");
    let state = stub_daemon_state("builder-a", exec_write_behavior, platform, supports_pty);
    spawn_named_daemon_on_listener(certs, listener, state.clone()).await;
    (addr, state)
```

Replace `spawn_named_daemon_on_addr` with:

```rust
pub(super) async fn spawn_named_daemon_on_listener(
    certs: &TestCerts,
    listener: tokio::net::TcpListener,
    state: StubDaemonState,
) {
    let addr = listener.local_addr().expect("read TLS stub daemon addr");
    let app = stub_router(state.clone());

    let daemon_config = remote_exec_daemon::config::DaemonConfig {
        target: state.target.clone(),
        listen: addr,
        default_workdir: PathBuf::from("."),
        windows_posix_root: None,
        transport: remote_exec_daemon::config::DaemonTransport::Tls,
        http_auth: None,
        sandbox: None,
        enable_transfer_compression: state.target_supports_transfer_compression,
        transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
        allow_login_shell: true,
        pty: remote_exec_daemon::config::PtyMode::Auto,
        default_shell: None,
        yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
        port_forward_limits: remote_exec_daemon::config::HostPortForwardLimits::default(),
        experimental_apply_patch_target_encoding_autodetect: false,
        process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
        tls: Some(remote_exec_daemon::config::TlsConfig {
            cert_pem: certs.daemon_cert.clone(),
            key_pem: certs.daemon_key.clone(),
            ca_pem: certs.ca_cert.clone(),
            pinned_client_cert_pem: None,
        }),
    };

    let task_state = state.clone();
    spawn_stub_task(&state, "tls-server", async move {
        remote_exec_daemon::test_support::serve_tls_on_listener(
            app,
            Arc::new(daemon_config),
            listener,
            std::future::pending::<()>(),
        )
        .await
    })
    .await;

    wait_until_ready(certs, addr).await;
    assert_no_stub_task_panics(&task_state).await;
}
```

This replaces the last TLS stub startup path that used a dropped `:0` address; keep the listener-taking `serve_tls_on_listener` call in this helper.

- [ ] **Step 3: Make dead and late target addresses explicit in spawners.**

In `crates/remote-exec-broker/tests/support/spawners.rs`, remove:

```rust
use super::certs::allocate_addr;
```

Add:

```rust
fn allocate_unbound_addr_for_dead_target() -> std::net::SocketAddr {
    // Intentionally unbound: these tests need broker startup to see a dead target.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr
}
```

Use it only in `spawn_broker_with_reverse_ordered_targets` and `spawn_broker_with_live_and_dead_targets`.

Change `DelayedTargetFixture` to retain a listener:

```rust
pub struct DelayedTargetFixture {
    pub broker: BrokerFixture,
    delayed_listener: tokio::sync::Mutex<Option<tokio::net::TcpListener>>,
}
```

Update `spawn_target`:

```rust
    pub async fn spawn_target(&self, target: &str) {
        let listener = self
            .delayed_listener
            .lock()
            .await
            .take()
            .expect("late target listener should only be consumed once");
        spawn_plain_http_stub_on_listener(
            listener,
            stub_daemon_state(target, ExecWriteBehavior::Success, "linux", true),
        )
        .await;
    }
```

In `spawn_broker_with_late_target`, replace `let delayed_addr = allocate_addr();` with:

```rust
    let delayed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let delayed_addr = delayed_listener.local_addr().unwrap();
```

Use a short startup probe for the late target so the pre-bound-but-not-serving listener does not slow the test:

```rust
            let late_target_extra_config = r#"[targets.builder-b.timeouts]
startup_probe_ms = 100"#;
            BrokerConfigTarget {
                name: "builder-b",
                addr: delayed_addr,
                transport: BrokerTargetTransport::Http,
                extra_config: Some(late_target_extra_config),
            },
```

Return:

```rust
    DelayedTargetFixture {
        broker: BrokerFixture {
            _tempdir: tempdir,
            client,
            stub_state,
        },
        delayed_listener: tokio::sync::Mutex::new(Some(delayed_listener)),
    }
```

- [ ] **Step 4: Use listener-taking daemon startup in multi-target Rust fixtures.**

In `crates/remote-exec-broker/tests/multi_target/support.rs`, change `DaemonFixture::spawn` to bind the backend listener before the proxy is created:

```rust
        let backend_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_addr = backend_listener.local_addr().unwrap();
```

Replace `fixture.start().await;` with:

```rust
        fixture.start_on_listener(backend_listener).await;
```

Replace `start` with a listener-taking helper:

```rust
    async fn start_on_listener(&mut self, listener: tokio::net::TcpListener) {
        let config = self.daemon_config();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown = Some(shutdown_tx);
        self.handle = Some(tokio::spawn(remote_exec_daemon::test_support::run_until_on_listener(
            config,
            listener,
            async move {
                let _ = shutdown_rx.await;
            },
        )));
        wait_until_ready_http(&self.client, self.addr).await;
    }

    async fn start(&mut self) {
        let listener = tokio::net::TcpListener::bind(self.backend_addr)
            .await
            .expect("rebind daemon backend listener");
        self.start_on_listener(listener).await;
    }
```

Move the existing config literal into:

```rust
    fn daemon_config(&self) -> remote_exec_daemon::config::DaemonConfig {
        remote_exec_daemon::config::DaemonConfig {
            target: self.target.clone(),
            listen: self.backend_addr,
            default_workdir: self.workdir.clone(),
            windows_posix_root: None,
            transport: remote_exec_daemon::config::DaemonTransport::Http,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::Auto,
            default_shell: None,
            yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
            port_forward_limits: remote_exec_daemon::config::HostPortForwardLimits::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: None,
        }
    }
```

Delete `pub fn allocate_addr()` from this file after the remote-listener tests stop using it.

- [ ] **Step 5: Make remote listener release tests use returned endpoints.**

In `crates/remote-exec-broker/tests/multi_target.rs`, in `forward_ports_release_remote_listeners_when_broker_stops`, remove:

```rust
    let listen_addr = support::allocate_addr();
```

Use `"127.0.0.1:0"` in the open request:

```rust
                    "listen_endpoint": "127.0.0.1:0",
```

Read the actual endpoint from the response:

```rust
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .expect("listen endpoint")
        .to_string();
    assert_ne!(listen_endpoint, "127.0.0.1:0");
```

Then wait on the returned endpoint:

```rust
    support::wait_for_daemon_listener_rebind(&listen_endpoint, Duration::from_secs(10)).await;
```

In `forward_ports_release_remote_listeners_after_broker_crash`, do the same for the first open. Remove:

```rust
    let listen_addr = support::allocate_addr();
```

Use `listen_endpoint.clone()` for the reopen request and assertions:

```rust
                    "listen_endpoint": listen_endpoint,
```

- [ ] **Step 6: Run targeted verification.**

Run:

```bash
! rg -n "allocate_addr\\(" crates/remote-exec-broker/tests/support/certs.rs crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/multi_target.rs
cargo test -p remote-exec-broker --test mcp_exec broker_rejects_unverified_target_if_it_returns_as_the_wrong_daemon -- --nocapture
cargo test -p remote-exec-broker --test mcp_exec list_targets_repopulates_cached_daemon_info_after_later_successful_verification -- --nocapture
cargo test -p remote-exec-broker --test multi_target forward_ports_release_remote_listeners_when_broker_stops -- --nocapture
```

Expected: the `rg` command exits with no matches. All test commands pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-broker/tests/support/certs.rs crates/remote-exec-broker/tests/support/stub_daemon.rs crates/remote-exec-broker/tests/support/spawners.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/multi_target.rs
git commit -m "test: use retained listeners in rust fixtures"
```

### Task 8: Add Broker Bound-Address Test Handoff

**Finding:** D4 `#27`

**Files:**
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_assets list_targets_includes_enabled_local_target`
  - `cargo test -p remote-exec-broker --test multi_target forward_ports_release_remote_listeners_after_broker_crash -- --nocapture`

**Testing approach:** existing process-fixture tests
Reason: Some tests intentionally kill the broker child process, so in-process listener injection is the wrong seam. A hidden bound-address file lets the child own `bind(0)` and gives tests the actual endpoint without dropped-port allocation.

- [ ] **Step 1: Write the streamable HTTP bound address when test env is set.**

In `crates/remote-exec-broker/src/mcp_server.rs`, add:

```rust
async fn write_test_bound_addr_file(local_addr: std::net::SocketAddr) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE") else {
        return Ok(());
    };
    tokio::fs::write(&path, format!("{local_addr}\n"))
        .await
        .with_context(|| {
            format!(
                "writing broker test bound address file {}",
                std::path::Path::new(&path).display()
            )
        })
}
```

After `local_addr` is read in `serve_streamable_http`, call:

```rust
    write_test_bound_addr_file(local_addr).await?;
```

Keep this hook undocumented in `README.md`; it is a test fixture hook, not an operator contract.

- [ ] **Step 2: Add a bound-address file waiter for Rust broker fixtures.**

In `crates/remote-exec-broker/tests/support/spawners.rs`, add `use std::time::Duration;` near the other `std` imports, then add:

```rust
async fn wait_for_bound_addr_file(path: &Path, resource: &str) -> std::net::SocketAddr {
    let started = std::time::Instant::now();
    let mut last = String::new();
    loop {
        match tokio::fs::read_to_string(path).await {
            Ok(value) => match value.trim().parse() {
                Ok(addr) => return addr,
                Err(err) => last = format!("invalid address `{}`: {err}", value.trim()),
            },
            Err(err) => last = err.to_string(),
        }
        if started.elapsed() >= Duration::from_secs(5) {
            panic!("{resource} did not write bound address file {}; last={last}", path.display());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
```

- [ ] **Step 3: Use child-owned `127.0.0.1:0` in support streamable broker fixture.**

In `spawn_streamable_http_broker_with_stub_daemon`, remove `let broker_addr = allocate_addr();`.

Add:

```rust
    let bound_addr_file = tempdir.path().join("broker-bound-addr.txt");
```

Write the config with:

```rust
listen = {listen}
```

where:

```rust
listen = toml_string("127.0.0.1:0"),
```

Before spawn:

```rust
    command.env(
        "REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE",
        &bound_addr_file,
    );
```

After spawn:

```rust
    let broker_addr = wait_for_bound_addr_file(&bound_addr_file, "broker streamable HTTP").await;
    let url = format!("http://{broker_addr}/mcp");
```

Keep the readiness wait on `url`.

- [ ] **Step 4: Use the same handoff in multi-target `HttpBrokerFixture`.**

In `crates/remote-exec-broker/tests/multi_target/support.rs`, add the same bound-address helper:

```rust
async fn wait_for_bound_addr_file(path: &Path, resource: &str) -> std::net::SocketAddr {
    let started = std::time::Instant::now();
    let mut last = String::new();
    loop {
        match tokio::fs::read_to_string(path).await {
            Ok(value) => match value.trim().parse() {
                Ok(addr) => return addr,
                Err(err) => last = format!("invalid address `{}`: {err}", value.trim()),
            },
            Err(err) => last = err.to_string(),
        }
        if started.elapsed() >= Duration::from_secs(5) {
            panic!("{resource} did not write bound address file {}; last={last}", path.display());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
```

In `HttpBrokerFixture::spawn`, remove `let broker_addr = allocate_addr();`, write:

```rust
listen = "127.0.0.1:0"
```

in the generated config, set:

```rust
        let bound_addr_file = tempdir.path().join("broker-bound-addr.txt");
        command.env("REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE", &bound_addr_file);
```

then after spawning:

```rust
        let broker_addr = wait_for_bound_addr_file(&bound_addr_file, "multi-target broker").await;
        let url = format!("http://{broker_addr}/mcp");
```

- [ ] **Step 5: Run targeted verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_assets list_targets_includes_enabled_local_target
cargo test -p remote-exec-broker --test multi_target forward_ports_release_remote_listeners_after_broker_crash -- --nocapture
```

Expected: all commands pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/mcp_server.rs crates/remote-exec-broker/tests/support/spawners.rs crates/remote-exec-broker/tests/multi_target/support.rs
git commit -m "test: report broker child bound address"
```

### Task 9: Add C++ Daemon Bound-Address Test Handoff

**Finding:** D4 `#27`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `! rg -n "allocate_addr\\(" crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp real_cpp_daemon_releases_listener_after_broker_crash -- --nocapture`

**Testing approach:** C++ build/test + broker-to-C++ integration test
Reason: The C++ daemon is a child process, so Rust cannot pass it a live listener. Letting the daemon bind port `0` and write its actual bound address removes the TOCTOU while preserving the real process path.

- [ ] **Step 1: Add hidden C++ config field and allow port zero only with that field.**

In `crates/remote-exec-daemon-cpp/include/config.h`, add to `DaemonConfig`:

```cpp
    std::string test_bound_addr_file;
```

In `crates/remote-exec-daemon-cpp/src/config.cpp`, replace `read_listen_port` with:

```cpp
static int read_listen_port(const ConfigValues& values) {
    const unsigned long listen_port =
        parse_unsigned_long(read_required_string(values, "listen_port"), "listen_port");
    const bool test_bound_addr_file_present =
        values.find("test_bound_addr_file") != values.end();
    if (listen_port > 65535UL || (listen_port == 0UL && !test_bound_addr_file_present)) {
        throw std::runtime_error("listen_port must be between 1 and 65535");
    }
    return static_cast<int>(listen_port);
}
```

In `load_config`, add:

```cpp
    config.test_bound_addr_file = read_optional_string(values, "test_bound_addr_file", "");
```

Add a validation guard:

```cpp
    if (config.listen_port == 0 && config.test_bound_addr_file.empty()) {
        throw std::runtime_error("listen_port = 0 requires test_bound_addr_file");
    }
```

- [ ] **Step 2: Write the actual C++ daemon bound address after bind.**

In `crates/remote-exec-daemon-cpp/src/server.cpp`, add includes:

```cpp
#include <fstream>
#include <stdexcept>
```

Add helper:

```cpp
static void write_test_bound_addr_file(const DaemonConfig& config, unsigned short bound_port) {
    if (config.test_bound_addr_file.empty()) {
        return;
    }
    std::ofstream out(config.test_bound_addr_file.c_str(), std::ios::out | std::ios::trunc);
    if (!out) {
        throw std::runtime_error("failed to open test_bound_addr_file");
    }
    out << config.listen_host << ':' << bound_port << '\n';
    if (!out) {
        throw std::runtime_error("failed to write test_bound_addr_file");
    }
}
```

In `run_server`, after `runtime.start_accept_loop();`, add:

```cpp
    const unsigned short bound_port = runtime.bound_port();
    write_test_bound_addr_file(runtime.state().config, bound_port);
```

Use `bound_port` in the log message instead of calling `runtime.bound_port()` again.

- [ ] **Step 3: Add bound-address file helpers in the C++ broker test.**

In `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`, add:

```rust
async fn wait_for_bound_addr_file(path: &Path, resource: &str) -> std::net::SocketAddr {
    let started = std::time::Instant::now();
    let mut last = String::new();
    loop {
        match tokio::fs::read_to_string(path).await {
            Ok(value) => match value.trim().parse() {
                Ok(addr) => return addr,
                Err(err) => last = format!("invalid address `{}`: {err}", value.trim()),
            },
            Err(err) => last = err.to_string(),
        }
        if started.elapsed() >= Duration::from_secs(5) {
            panic!("{resource} did not write bound address file {}; last={last}", path.display());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn spawn_cpp_daemon_with_bound_addr(
    daemon_binary: &Path,
    daemon_config: &Path,
    bound_addr_file: &Path,
    config_body: String,
) -> (tokio::process::Child, std::net::SocketAddr) {
    std::fs::write(daemon_config, config_body).unwrap();
    let mut daemon = tokio::process::Command::new(daemon_binary);
    daemon.arg(daemon_config);
    apply_quiet_test_logging(&mut daemon);
    let child = spawn_cpp_daemon_process(&mut daemon).await;
    let daemon_addr = wait_for_bound_addr_file(bound_addr_file, "C++ daemon").await;
    wait_until_ready_http(daemon_addr).await;
    (child, daemon_addr)
}
```

Delete the local `allocate_addr()` helper after all call sites are removed.

- [ ] **Step 4: Update `CppDaemonBrokerFixture` to let C++ bind port zero.**

In `CppDaemonBrokerFixture::spawn_with_daemon_config`, replace the dropped `daemon_addr` and `backend_addr` selection with:

```rust
        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
```

Write the C++ daemon config with:

```rust
        let daemon_config_body = format!(
            "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n{}",
            daemon_workdir.display(),
            daemon_bound_addr_file.display(),
            extra_daemon_config
        );
        let (daemon, backend_addr) = spawn_cpp_daemon_with_bound_addr(
            &daemon_binary,
            &daemon_config,
            &daemon_bound_addr_file,
            daemon_config_body,
        )
        .await;
```

Start the proxy after the daemon address is known. Change `TunnelDropProxy::spawn` to bind its own listener:

```rust
    async fn spawn(daemon_addr: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen_addr = listener.local_addr().unwrap();
        ...
    }
```

Add `listen_addr: std::net::SocketAddr` to `TunnelDropProxy`, set it in `spawn`, and use `let daemon_addr = proxy.listen_addr;` for the broker target config. The proxy still forwards to `backend_addr`.

- [ ] **Step 5: Update crashable C++ fixture to let both children bind port zero.**

In `CrashableCppDaemonBrokerFixture::spawn`, replace dropped `daemon_addr` and `broker_addr` with bound-address files:

```rust
        let daemon_bound_addr_file = tempdir.path().join("daemon-bound-addr.txt");
        let broker_bound_addr_file = tempdir.path().join("broker-bound-addr.txt");
```

Write daemon config with `listen_port = 0` and `test_bound_addr_file = ...`, then:

```rust
        let daemon_config_body = format!(
            "target = builder-cpp\nlisten_host = 127.0.0.1\nlisten_port = 0\ndefault_workdir = {}\ntest_bound_addr_file = {}\n",
            daemon_workdir.display(),
            daemon_bound_addr_file.display()
        );
        let (daemon, daemon_addr) = spawn_cpp_daemon_with_bound_addr(
            &daemon_binary,
            &daemon_config,
            &daemon_bound_addr_file,
            daemon_config_body,
        )
        .await;
```

Write broker config with:

```toml
listen = "127.0.0.1:0"
```

Set before spawning the broker:

```rust
        broker.env("REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE", &broker_bound_addr_file);
```

After spawn:

```rust
        let broker_addr = wait_for_bound_addr_file(&broker_bound_addr_file, "C++ broker").await;
        let broker_url = format!("http://{broker_addr}/mcp");
```

- [ ] **Step 6: Use returned remote listener endpoints in the C++ crash test.**

In `real_cpp_daemon_releases_listener_after_broker_crash`, remove:

```rust
    let listen_addr = allocate_addr();
```

Open with:

```rust
listen_endpoint: "127.0.0.1:0".to_string(),
```

Then capture:

```rust
    let listen_endpoint = open.structured_content["forwards"][0]["listen_endpoint"]
        .as_str()
        .expect("listen endpoint")
        .to_string();
    assert_ne!(listen_endpoint, "127.0.0.1:0");
```

Use `&listen_endpoint` in `wait_for_public_forward_reopen`.

- [ ] **Step 7: Run targeted verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
! rg -n "allocate_addr\\(" crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp real_cpp_daemon_releases_listener_after_broker_crash -- --nocapture
```

Expected: `check-posix` passes, `rg` exits with no matches, and the C++ integration test passes.

- [ ] **Step 8: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/config.h crates/remote-exec-daemon-cpp/src/config.cpp crates/remote-exec-daemon-cpp/src/server.cpp crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "test: report cpp daemon bound address"
```

### Task 10: Replace Rust Port-Forward Sleeps And UDP Timing Windows With Observable Waits

**Findings:** D4 `#30`, D4 `#31`

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target.rs`
- Modify: `crates/remote-exec-broker/tests/multi_target/support.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_forward_open_after_stream_connect_error`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_retries_udp_connector_after_bind_error`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_drops_udp_datagrams_under_pressure`
  - `cargo test -p remote-exec-broker --test multi_target forward_ports_reconnect_after_connect_side_tunnel_drop_and_accept_new_tcp_connections`
  - `cargo test -p remote-exec-broker --test multi_target forward_ports_reconnect_after_connect_side_tunnel_drop_and_relays_future_udp_datagrams`

**Testing approach:** existing tests + targeted verification
Reason: The tests already assert the desired product behavior. D4 changes their synchronization to wait for counters/health/positive echoes instead of sleeping or relying on tight negative timing. This task must remove every fixed 200/250 ms synchronization sleep and every 100 ms UDP negative receive window in the listed Rust files, while preserving short polling sleeps inside named wait helpers.

- [ ] **Step 1: Replace stream-connect-error sleep in `mcp_forward_ports.rs`.**

In `forward_ports_keeps_forward_open_after_stream_connect_error`, replace:

```rust
    tokio::time::sleep(Duration::from_millis(250)).await;
    let listed = list_forward(&fixture, &forward_id).await;
```

with:

```rust
    let listed = wait_for_forward_status(&fixture, &forward_id, "open", Duration::from_secs(5)).await;
```

Keep the `last_error` assertion.

- [ ] **Step 2: Widen UDP negative retry windows and keep positive proof.**

In `forward_ports_retries_udp_connector_after_bind_error`, replace each inner `Duration::from_millis(100)` receive timeout with:

```rust
Duration::from_millis(500)
```

This loop already sends `"second"` repeatedly and requires a positive echo before passing, so the wider window reduces false-negative timing without weakening the assertion.

In `send_udp_until_echo`, rename the inner receive window constant so it is clearly polling backoff rather than a negative assertion:

```rust
const UDP_ECHO_POLL_WINDOW: Duration = Duration::from_millis(100);
```

Then use `UDP_ECHO_POLL_WINDOW` in the `tokio::time::timeout` call. Do not leave an inline `Duration::from_millis(100)` next to `recv_from`.

In `forward_ports_drops_udp_datagrams_under_pressure`, add a positive control after `wait_for_udp_drop_count` by opening a second normal UDP forward to a local echo socket and calling existing `send_udp_until_echo(...)`. Because the pressure forward intentionally targets `127.0.0.1:9`, create a separate normal UDP forward in the same fixture:

```rust
let echo_socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
let echo_addr = echo_socket.local_addr().unwrap();
tokio::spawn(async move {
    let mut buf = [0u8; 1024];
    loop {
        let (read, peer) = match echo_socket.recv_from(&mut buf).await {
            Ok(value) => value,
            Err(_) => return,
        };
        if echo_socket.send_to(&buf[..read], peer).await.is_err() {
            return;
        }
    }
});

let positive = open_udp_forward(&fixture, "local", "local", echo_addr).await;
let positive_forward_id = forward_id_from(&positive);
let positive_listen_endpoint = listen_endpoint_from(&positive);
let positive_client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
send_udp_until_echo(
    &positive_client,
    &positive_listen_endpoint,
    b"post-drop-positive",
    Duration::from_secs(5),
    "positive UDP echo should still arrive after drop accounting",
)
.await;
let positive_close = close_forward(&fixture, positive_forward_id).await;
assert_eq!(positive_close.structured_content["forwards"][0]["status"], "closed");
```

- [ ] **Step 3: Replace multi-target reconnect-settle sleeps with ready waits.**

In `multi_target.rs`, replace the `tokio::time::sleep(Duration::from_millis(250)).await;` after successful `"after"` TCP echo with:

```rust
let forward = support::wait_for_forward_ready_after_reconnect(
    &cluster.broker,
    &forward_id,
    Duration::from_secs(5),
)
.await;
assert_eq!(forward["status"], "open");
assert_eq!(forward["phase"], "ready");
```

Do the same in the UDP reconnect test after the `"after"` echo. Remove the later duplicate ready wait and keep the dropped-counter assertion from this `forward` value.

- [ ] **Step 4: Replace broker-crash sleep with an observed remote listener wait.**

In `multi_target.rs`, in `forward_ports_release_remote_listeners_after_broker_crash`, replace:

```rust
    tokio::time::sleep(Duration::from_millis(200)).await;
    broker.kill().await;
```

with an assertion against the returned endpoint, then kill immediately. The open result already proves the listener was created, so no sleep is needed:

```rust
    assert_eq!(
        open.structured_content["forwards"][0]["listen_endpoint"].as_str(),
        Some(listen_endpoint.as_str())
    );
    broker.kill().await;
```

If this assertion already exists after the sleep, move it before `broker.kill()` and delete the sleep.

- [ ] **Step 5: Replace C++ reconnect-settle sleeps.**

In `mcp_forward_ports_cpp.rs`, add this helper near `CppDaemonBrokerFixture::list_forward`:

```rust
async fn wait_for_forward_ready(
    client: &RemoteExecClient,
    forward_id: &str,
    timeout: Duration,
) -> serde_json::Value {
    let started = std::time::Instant::now();
    loop {
        let response = client
            .call_tool(
                "forward_ports",
                &ForwardPortsInput::List {
                    forward_ids: vec![forward_id.to_string()],
                    listen_side: None,
                    connect_side: None,
                },
            )
            .await
            .unwrap();
        let entry = response.structured_content["forwards"][0].clone();
        if entry["status"] == "open" && entry["phase"] == "ready" {
            return entry;
        }
        if started.elapsed() >= timeout {
            panic!("forward `{forward_id}` did not become ready within {timeout:?}; last={entry}");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
```

Use this helper instead of fixed sleeps after tunnel drops.

- [ ] **Step 6: Classify and remove the remaining Rust fixed sleeps in the D4 port-forward files.**

Scan the affected Rust files and classify every remaining sleep:

```bash
rg -n "tokio::time::sleep\\(Duration::from_millis\\((200|250)\\)" crates/remote-exec-broker/tests/mcp_forward_ports.rs crates/remote-exec-broker/tests/multi_target.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
rg -n "timeout\\(Duration::from_millis\\(100\\).*recv_from" crates/remote-exec-broker/tests/mcp_forward_ports.rs
```

Handle the current matches as follows:

- `crates/remote-exec-broker/tests/mcp_forward_ports.rs:183`: replace with `wait_for_forward_status`.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs:1209`: widen to `Duration::from_millis(500)` and keep the positive `"second"` echo requirement.
- `crates/remote-exec-broker/tests/mcp_forward_ports.rs:1608`: replace with `UDP_ECHO_POLL_WINDOW` inside `send_udp_until_echo`, with the positive echo assertion preserved by the helper.
- `crates/remote-exec-broker/tests/multi_target.rs:342`: replace with `wait_for_forward_ready_after_reconnect`.
- `crates/remote-exec-broker/tests/multi_target.rs:463`: replace with `wait_for_forward_ready_after_reconnect`.
- `crates/remote-exec-broker/tests/multi_target.rs:561`: delete after moving the listener-endpoint assertion before `broker.kill()`.
- `crates/remote-exec-broker/tests/multi_target.rs:952`: keep only if it is inside an explicit listener-close polling helper; otherwise replace with that helper's named polling interval constant.
- `crates/remote-exec-broker/tests/multi_target/support.rs:842`: keep only if it is in a named listener-close polling helper with an outer timeout and endpoint-specific panic.
- `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs:390`, `:436`, and `:770`: replace with `wait_for_forward_ready`, listener-endpoint assertions, or the C++ fixture's named polling helper.

- [ ] **Step 7: Verify no unclassified fixed synchronization sleeps or inline 100 ms UDP negative windows remain.**

Run:

```bash
! rg -n "tokio::time::sleep\\(Duration::from_millis\\((200|250)\\)" crates/remote-exec-broker/tests/mcp_forward_ports.rs crates/remote-exec-broker/tests/multi_target.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
! rg -n "timeout\\(Duration::from_millis\\(100\\).*recv_from" crates/remote-exec-broker/tests/mcp_forward_ports.rs
```

Expected: both commands exit successfully with no matches.

- [ ] **Step 8: Run targeted verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_keeps_forward_open_after_stream_connect_error
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_retries_udp_connector_after_bind_error
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_drops_udp_datagrams_under_pressure
cargo test -p remote-exec-broker --test multi_target forward_ports_reconnect_after_connect_side_tunnel_drop_and_accept_new_tcp_connections
cargo test -p remote-exec-broker --test multi_target forward_ports_reconnect_after_connect_side_tunnel_drop_and_relays_future_udp_datagrams
```

Expected: all commands pass.

- [ ] **Step 9: Commit.**

```bash
git add crates/remote-exec-broker/tests/mcp_forward_ports.rs crates/remote-exec-broker/tests/multi_target.rs crates/remote-exec-broker/tests/multi_target/support.rs crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
git commit -m "test: replace port forward sleeps with waits"
```

### Task 11: Replace C++ Test Sleeps With Bounded Conditions

**Finding:** D4 `#30`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-session-store`
  - `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** existing C++ tests + targeted verification
Reason: The same tests should prove the same behavior, but synchronization should poll for the condition they need instead of assuming a fixed runtime delay. This task must classify every current `platform::sleep_ms` in the listed C++ tests and leave only named polling backoffs or explicit timeout-margin waits whose purpose is encoded in a helper name or local constant.

- [ ] **Step 1: Add bounded polling helper in `test_session_store.cpp`.**

Near `zombie_children_of_current_process`, add:

```cpp
static bool wait_until_zombie_delta_at_most(
    unsigned long baseline,
    unsigned long allowed_delta,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (zombie_children_of_current_process() <= baseline + allowed_delta) {
            return true;
        }
        platform::sleep_ms(25UL);
    }
    return zombie_children_of_current_process() <= baseline + allowed_delta;
}
```

Replace the fixed child-reaper wait in `assert_posix_sigchld_reaper_reaps_exited_session_children`:

```cpp
    platform::sleep_ms(400UL);
    for (int attempt = 0; attempt < 40; ++attempt) {
        if (zombie_children_of_current_process() <= baseline_zombies) {
            return;
        }
        platform::sleep_ms(25UL);
    }
    assert(zombie_children_of_current_process() <= baseline_zombies);
```

with:

```cpp
    assert(wait_until_zombie_delta_at_most(baseline_zombies, 0UL, 2000UL));
```

Also replace the current `platform::sleep_ms(200)` used to let the slow `write_stdin` thread start with a bounded condition:

```cpp
static bool wait_until_true(const std::atomic<bool>& value, unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (value.load()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return value.load();
}
```

Set an `std::atomic<bool> slow_thread_started(false);` at the start of the thread body, then assert it:

```cpp
        std::atomic<bool> slow_thread_started(false);
        std::thread slow_thread([&]() {
            slow_thread_started.store(true);
            slow_poll = store.write_stdin(
                slow_running.at("daemon_session_id").get<std::string>(),
                "",
                true,
                5000UL,
                DEFAULT_MAX_OUTPUT_TOKENS,
                yield_time,
                false,
                0U,
                0U
            );
        });

        assert(wait_until_true(slow_thread_started, 1000UL));
```

Replace the current `platform::sleep_ms(150UL)` before starting the replacement session with a bounded poll that waits until the exited session can be pruned:

```cpp
static bool wait_until_session_exits(
    SessionStore& store,
    const std::string& session_id,
    const YieldTimeConfig& yield_time,
    unsigned long timeout_ms
) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        const Json poll = store.write_stdin(
            session_id,
            "",
            true,
            1UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            false,
            0U,
            0U
        );
        if (!poll.at("running").get<bool>()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return false;
}
```

Use it with the exited session ID before starting the replacement session:

```cpp
        assert(wait_until_session_exits(
            exited_store,
            exited_running.at("daemon_session_id").get<std::string>(),
            fast_yield,
            2000UL
        ));
```

- [ ] **Step 2: Replace long socket-buffer sleep in `test_server_streaming.cpp`.**

Find the test that uses `platform::sleep_ms(5000)` to hold a socket buffer full. Replace the sleep with a condition variable or atomic flag:

```cpp
std::atomic<bool> sender_blocked(false);
std::atomic<bool> release_sender(false);
std::thread blocker([&]() {
    sender_blocked.store(true);
    while (!release_sender.load()) {
        platform::sleep_ms(10UL);
    }
});
assert(wait_until_true(sender_blocked, 1000UL));
```

If there is no existing `wait_until_true`, add:

```cpp
static bool wait_until_true(const std::atomic<bool>& value, unsigned long timeout_ms) {
    const std::uint64_t started = platform::monotonic_ms();
    while (platform::monotonic_ms() - started < timeout_ms) {
        if (value.load()) {
            return true;
        }
        platform::sleep_ms(10UL);
    }
    return value.load();
}
```

Set `release_sender = true` during cleanup and join the thread.

- [ ] **Step 3: Classify the remaining C++ sleeps and remove fixed synchronization sleeps.**

Run:

```bash
rg -n "platform::sleep_ms" crates/remote-exec-daemon-cpp/tests/test_session_store.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
```

Handle the current matches as follows:

- `test_session_store.cpp:435`: replace with `wait_until_zombie_delta_at_most`.
- `test_session_store.cpp:440`: keep only inside `wait_until_zombie_delta_at_most` as polling backoff.
- `test_session_store.cpp:562`: replace with `wait_until_true(slow_thread_started, 1000UL)`.
- `test_session_store.cpp:887`: replace with `wait_until_session_exits(...)`.
- `test_server_streaming.cpp:598`: keep only inside `wait_until_bindable`, which is a bounded polling helper.
- `test_server_streaming.cpp:1313`: replace the post-send sleep in `accept_and_send_tcp_payload` with a socket close/flush condition or a named `TCP_PAYLOAD_DRAIN_MARGIN_MS` constant if the test requires a deliberate peer-hold margin.
- `test_server_streaming.cpp:1580`: replace with the `sender_blocked`/`release_sender` synchronization from Step 2.
- `test_server_streaming.cpp:1730`: replace the raw `resume_timeout_ms + 200UL` sleep with a helper named `wait_past_resume_timeout(resume_timeout_ms)` that computes the margin internally and documents why this is a protocol timeout boundary rather than generic synchronization.

- [ ] **Step 4: Verify no unclassified C++ sleeps remain.**

Run:

```bash
! rg -n "platform::sleep_ms\\((150UL|200|400UL|5000UL)\\)" crates/remote-exec-daemon-cpp/tests/test_session_store.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
rg -n "wait_until_zombie_delta_at_most|wait_until_true|wait_until_session_exits|wait_past_resume_timeout|TCP_PAYLOAD_DRAIN_MARGIN_MS" crates/remote-exec-daemon-cpp/tests/test_session_store.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
```

Expected: the first command exits successfully with no matches, and the second command shows the named helpers/constants that justify the remaining waits.

- [ ] **Step 5: Run focused C++ verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: both commands pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/tests/test_session_store.cpp crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "test: replace cpp sleeps with bounded waits"
```

### Task 12: Add Host No-Default-Features CI Coverage

**Finding:** D4 `#33`

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify only if verification exposes cfg fallout: affected Rust files under `crates/remote-exec-host/src/`
- Test/Verify:
  - `cargo test -p remote-exec-host --no-default-features --tests`
  - `cargo clippy -p remote-exec-host --no-default-features --all-targets -- -D warnings`

**Testing approach:** CI command parity + local verification
Reason: The bug is missing coverage, not a runtime behavior gap. The new commands themselves are the regression guard; if they expose cfg errors, fix those minimal errors in the host crate.

- [ ] **Step 1: Run the missing local commands before editing CI.**

Run:

```bash
cargo test -p remote-exec-host --no-default-features --tests
cargo clippy -p remote-exec-host --no-default-features --all-targets -- -D warnings
```

Expected: commands may pass or expose host cfg issues. If they fail, capture the exact compiler/lint output and fix only the no-default-feature fallout before continuing.

- [ ] **Step 2: Add CI test step.**

In `.github/workflows/ci.yml`, under `rust-test-no-default-features`, after daemon:

```yaml
      - name: Test host without default features
        run: cargo test -p remote-exec-host --no-default-features --tests --locked
```

- [ ] **Step 3: Add CI clippy step.**

Under `rust-clippy-no-default-features`, after daemon:

```yaml
      - name: Run host clippy without default features
        run: cargo clippy -p remote-exec-host --no-default-features --all-targets --locked -- -D warnings
```

- [ ] **Step 4: Update README focused commands and CI note.**

In `README.md`, update the "Focused no-default-features commands" block to include:

```bash
cargo test -p remote-exec-host --no-default-features --tests
cargo clippy -p remote-exec-host --no-default-features --all-targets -- -D warnings
```

Update the CI note near the bottom from "broker and daemon" to:

```text
CI also exercises broker, daemon, and host `--no-default-features` test and clippy jobs on Ubuntu so the `tls-disabled` and host feature-gated code paths stay intentionally covered.
```

- [ ] **Step 5: Re-run verification.**

Run:

```bash
cargo test -p remote-exec-host --no-default-features --tests
cargo clippy -p remote-exec-host --no-default-features --all-targets -- -D warnings
```

Expected: both commands pass.

- [ ] **Step 6: Commit.**

```bash
git add .github/workflows/ci.yml README.md crates/remote-exec-host/src
git commit -m "ci: cover host without default features"
```

### Task 13: Run XP Tests Under Wine When Available

**Finding:** D4 `#32`

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`
  - `command -v wine >/dev/null && make -C crates/remote-exec-daemon-cpp test-wine-session-store test-wine-transfer || true`

**Testing approach:** CI command parity + local conditional verification
Reason: XP binaries are already built; D4 ensures CI runs them when a Linux runner has Wine installed. Local verification should be conditional because Wine may not be installed in every development environment.

- [ ] **Step 1: Add Wine package to Linux C++ dependencies.**

In `.github/workflows/ci.yml`, update the Linux C++ dependency install line:

```yaml
          sudo apt-get install -y g++ make g++-mingw-w64-i686 wine
```

- [ ] **Step 2: Run XP Wine tests after XP build in CI.**

In the Linux C++ checks shell block, after both `wait` calls and the failure check for `check-posix` / `check-windows-xp`, add:

```bash
          if command -v wine >/dev/null 2>&1; then
            make -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-windows-xp test-wine-session-store test-wine-transfer
          else
            echo "wine not available; skipping XP runtime tests"
          fi
```

Keep this after the compile checks so build failures remain easy to distinguish.

- [ ] **Step 3: Document XP runtime coverage.**

In `crates/remote-exec-daemon-cpp/README.md`, change the Windows XP cross-build bullets to:

```text
- `make all-windows-xp`
- `make check-windows-xp`
- `make test-wine-session-store` and `make test-wine-transfer` when `wine` is available; CI runs these on Linux after the XP cross-build.
```

- [ ] **Step 4: Run local verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-windows-xp
command -v wine >/dev/null && make -C crates/remote-exec-daemon-cpp test-wine-session-store test-wine-transfer || true
```

Expected: `check-windows-xp` passes. If Wine is installed, both Wine test targets pass; if Wine is absent, the second command exits successfully without running them.

- [ ] **Step 5: Commit.**

```bash
git add .github/workflows/ci.yml crates/remote-exec-daemon-cpp/README.md
git commit -m "ci: run xp tests under wine"
```

### Task 14: Run C++ Daemon Runtime Forwarding Natively On Windows

**Finding:** D4 `#36`

**Files:**
- Create: `crates/remote-exec-daemon-cpp/mk/windows-native.mk`
- Modify: `crates/remote-exec-daemon-cpp/GNUmakefile`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Test/Verify:
  - Local Windows command: `make -C crates/remote-exec-daemon-cpp all-windows-native`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp list_targets_reports_port_forward_protocol_version_for_real_cpp_daemon`
  - CI command on `windows-latest`: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp windows_cpp_daemon_smoke -- --nocapture`

**Testing approach:** native Windows CI integration smoke + existing Unix integration tests
Reason: The audit item is missing runtime coverage. Documentation is not enough for D4; CI must start a host-native Windows C++ daemon process and exercise the broker path against it on a native Windows runner.

- [ ] **Step 1: Add a host-native Windows GNU make target.**

Create `crates/remote-exec-daemon-cpp/mk/windows-native.mk`:

```make
WINDOWS_NATIVE_CXX ?= g++

WINDOWS_NATIVE_PROD_OBJ_DIR := $(OBJ_DIR)/windows-native-prod
WINDOWS_NATIVE_TARGET := $(BUILD_DIR)/remote-exec-daemon-cpp.exe

WINDOWS_NATIVE_PROD_CPPFLAGS := $(COMMON_CPPFLAGS)
WINDOWS_NATIVE_PROD_CXXFLAGS := $(PROD_CXXFLAGS)
WINDOWS_NATIVE_LDFLAGS ?=
WINDOWS_NATIVE_LDLIBS := -lws2_32

WINDOWS_NATIVE_SRCS := \
	$(BASE_SRCS) \
	$(MAKEFILE_DIR)src/main.cpp \
	$(MAKEFILE_DIR)src/process_session_win32.cpp \
	$(MAKEFILE_DIR)src/console_output.cpp \
	$(MAKEFILE_DIR)src/win32_error.cpp

WINDOWS_NATIVE_OBJS := $(sort $(call cpp_objs,$(WINDOWS_NATIVE_PROD_OBJ_DIR),$(WINDOWS_NATIVE_SRCS)))

DEP_FILES += \
	$(WINDOWS_NATIVE_OBJS:.o=.d)

all-windows-native: $(WINDOWS_NATIVE_TARGET)

$(WINDOWS_NATIVE_TARGET): $(WINDOWS_NATIVE_OBJS)
	mkdir -p $(dir $@)
	$(WINDOWS_NATIVE_CXX) $(WINDOWS_NATIVE_PROD_CXXFLAGS) $(WINDOWS_NATIVE_LDFLAGS) -o $@ $^ $(WINDOWS_NATIVE_LDLIBS)

$(WINDOWS_NATIVE_PROD_OBJ_DIR)/%.o: $(MAKEFILE_DIR)%.cpp
	mkdir -p $(dir $@)
	$(WINDOWS_NATIVE_CXX) $(WINDOWS_NATIVE_PROD_CPPFLAGS) $(WINDOWS_NATIVE_PROD_CXXFLAGS) $(DEPFLAGS) -c -o $@ $<

.PHONY: all-windows-native
```

Modify `crates/remote-exec-daemon-cpp/GNUmakefile` so Windows-native rules are included only on Windows:

```make
ifeq ($(OS),Windows_NT)
include $(MAKEFILE_DIR)mk/windows-native.mk
endif
```

Keep the default `all` and `check` targets unchanged so Linux still defaults to POSIX and Windows XP stays explicit.

- [ ] **Step 2: Make the C++ broker integration fixture platform-aware.**

In `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`, remove the file-level gate:

```rust
#![cfg(unix)]
```

Update `cpp_daemon_binary`:

```rust
fn cpp_daemon_binary() -> PathBuf {
    let binary = if cfg!(windows) {
        "build/remote-exec-daemon-cpp.exe"
    } else {
        "build/remote-exec-daemon-cpp"
    };
    cpp_daemon_dir().join(binary)
}
```

Update `stage_cpp_daemon_binary`:

```rust
fn stage_cpp_daemon_binary(tempdir: &Path) -> PathBuf {
    let staged_name = if cfg!(windows) {
        "remote-exec-daemon-cpp.exe"
    } else {
        "remote-exec-daemon-cpp"
    };
    let staged = tempdir.join(staged_name);
    std::fs::copy(cpp_daemon_binary(), &staged).unwrap();
    staged
}
```

Update `ensure_cpp_daemon_built` so Windows invokes the native target under MSYS2 and Unix invokes the existing POSIX target:

```rust
    let target = if cfg!(windows) {
        "all-windows-native"
    } else {
        "all-posix"
    };
    let status = tokio::process::Command::new("make")
        .arg(target)
        .current_dir(&cpp_daemon_dir)
        .status()
        .await
        .unwrap();
    assert!(status.success(), "failed to build remote-exec-daemon-cpp with {target}");
```

- [ ] **Step 3: Gate Unix-only C++ integration tests individually and add a Windows smoke test.**

Leave broad Unix coverage on Unix by adding `#[cfg(unix)]` above the tests that use Unix shell commands or crash/rebind assumptions. Apply this exact attribute pattern to at least these current tests:

```rust
#[cfg(unix)]
#[tokio::test]
async fn broker_prunes_cpp_exec_sessions_when_daemon_limit_is_reached() {
```

```rust
#[cfg(unix)]
#[tokio::test]
async fn real_cpp_daemon_releases_listener_after_broker_crash() {
```

Also gate any C++ integration test that sends Unix shell commands such as `sleep 30` or `printf ...; sleep ...` through `exec_command`. Keep pure TCP/UDP forwarding tests enabled on Windows when their helper code compiles after Steps 1-2.

Add a compile-time check for the required Windows test name before writing CI:

```bash
rg -n "#\\[cfg\\(windows\\)\\]|windows_cpp_daemon_smoke" crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs
```

Expected: output shows the Windows cfg and the smoke test.

Add this Windows-safe test that starts the native Windows C++ daemon and exercises actual broker-to-daemon runtime forwarding with no Unix shell dependency:

```rust
#[cfg(windows)]
#[tokio::test]
async fn windows_cpp_daemon_smoke() {
    let fixture = CppDaemonBrokerFixture::spawn().await;

    let target_info = fixture
        .client
        .call_tool("list_targets", &serde_json::json!({}))
        .await
        .unwrap();
    assert!(!target_info.is_error, "list_targets failed: {}", target_info.text_output);
    assert_eq!(
        target_info.structured_content["targets"][0]["daemon_info"]["port_forward_protocol_version"],
        4
    );

    let echo_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_addr = echo_listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match echo_listener.accept().await {
                Ok(value) => value,
                Err(_) => return,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                loop {
                    let read = match stream.read(&mut buf).await {
                        Ok(0) => return,
                        Ok(read) => read,
                        Err(_) => return,
                    };
                    if stream.write_all(&buf[..read]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    let open = fixture.open_tcp_forward(&echo_addr.to_string()).await;
    assert!(!open.is_error, "open failed: {}", open.text_output);
    let opened = &open.structured_content["forwards"][0];
    let forward_id = opened["forward_id"].as_str().unwrap().to_string();
    let listen_endpoint = opened["listen_endpoint"].as_str().unwrap().to_string();

    let mut stream = tokio::net::TcpStream::connect(&listen_endpoint).await.unwrap();
    stream.write_all(b"windows-cpp-forward").await.unwrap();
    let mut echoed = [0u8; 19];
    stream.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"windows-cpp-forward");

    let close = fixture.close_forward(forward_id).await;
    assert!(!close.is_error, "close failed: {}", close.text_output);
    assert_eq!(close.structured_content["forwards"][0]["status"], "closed");
}
```

If any existing tests are already Windows-safe after Tasks 9-10, keep them enabled on Windows, but the `windows_cpp_daemon_smoke` test is the required #36 native runner proof.

- [ ] **Step 4: Add native Windows CI execution.**

Modify `.github/workflows/ci.yml` in the `cpp` job's MSYS2 setup so Rust and Make are available from the same native Windows runner:

```yaml
      - name: Install Rust toolchain for Windows C++ integration smoke
        if: runner.os == 'Windows'
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust dependencies for Windows C++ integration smoke
        if: runner.os == 'Windows'
        uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true
          prefix-key: v1-windows-cpp-smoke
```

Then extend the Windows C++ checks step:

```yaml
      - name: Run Windows C++ checks
        if: runner.os == 'Windows'
        shell: msys2 {0}
        run: |
          jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)"
          make -j"${jobs}" -C crates/remote-exec-daemon-cpp check-windows-xp
          make -j"${jobs}" -C crates/remote-exec-daemon-cpp all-windows-native

      - name: Expose MinGW runtime DLLs
        if: runner.os == 'Windows'
        shell: bash
        run: echo "C:\\msys64\\mingw32\\bin" >> "$GITHUB_PATH"

      - name: Run native Windows C++ daemon broker smoke
        if: runner.os == 'Windows'
        shell: bash
        run: cargo test -p remote-exec-broker --test mcp_forward_ports_cpp windows_cpp_daemon_smoke -- --nocapture
```

Use `shell: bash` or the default PowerShell for the Cargo test, not `shell: msys2 {0}`, so the Rust test and spawned daemon execute as normal Windows processes. Keep `C:\msys64\mingw32\bin` on `PATH` for that step so the spawned MinGW-built `remote-exec-daemon-cpp.exe` can load its runtime DLLs.

- [ ] **Step 5: Update root README CI notes.**

In `README.md`, near the CI coverage notes, add:

```text
- The Rust broker and Rust daemon are exercised on Linux and Windows. The standalone C++ daemon is built on Linux and Windows, POSIX runtime tests run on Linux, Windows XP-compatible test binaries run under Wine on Linux when available, and `mcp_forward_ports_cpp.rs` includes a native `windows-latest` broker-to-C++ daemon smoke test against `remote-exec-daemon-cpp.exe`.
```

- [ ] **Step 6: Update C++ daemon README.**

In `crates/remote-exec-daemon-cpp/README.md`, after the Windows XP build section, add:

```text
Runtime coverage note: host-native POSIX C++ daemon runtime tests run on Unix. Windows XP-compatible binaries are compile-checked on Linux and Windows and are executed under Wine on Linux when Wine is available. CI also builds `build/remote-exec-daemon-cpp.exe` with host-native MinGW on `windows-latest`, exposes `C:\msys64\mingw32\bin` for the MinGW runtime DLLs, and runs the Rust broker `mcp_forward_ports_cpp::windows_cpp_daemon_smoke` integration test against that process.
```

- [ ] **Step 7: Verify #36 no longer has gap-only wording.**

Run:

```bash
rg -n "windows_cpp_daemon_smoke|all-windows-native|remote-exec-daemon-cpp\\.exe|mcp_forward_ports_cpp" .github/workflows/ci.yml README.md crates/remote-exec-daemon-cpp/README.md crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs crates/remote-exec-daemon-cpp/mk/windows-native.mk
! rg -n "Unix-only|Windows-native fixture is missing|cpp windows runtime test gap" README.md crates/remote-exec-daemon-cpp/README.md
```

Expected: the first command shows the native Windows test/build hooks, and the second command exits successfully with no stale gap-only wording.

- [ ] **Step 8: Run local verification for the platform you are on.**

On Linux/macOS, run:

```bash
make -C crates/remote-exec-daemon-cpp all-posix
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp list_targets_reports_port_forward_protocol_version_for_real_cpp_daemon
```

On Windows, run natively:

```bash
make -C crates/remote-exec-daemon-cpp all-windows-native
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp windows_cpp_daemon_smoke -- --nocapture
```

Expected: the local platform commands pass. The Windows command is mandatory in CI even when the implementer is not on Windows locally.

- [ ] **Step 9: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/mk/windows-native.mk crates/remote-exec-daemon-cpp/GNUmakefile crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs .github/workflows/ci.yml README.md crates/remote-exec-daemon-cpp/README.md
git commit -m "ci: run cpp daemon smoke natively on windows"
```

### Task 15: Final D4 Quality Gate

**Files:**
- Verify only unless the commands expose formatting or lint issues.
- Modify only files touched in Tasks 2-14 if formatting, lint, or test fixes are required.

**Testing approach:** focused D4 verification + full workspace quality gate
Reason: D4 touches broker test infrastructure, Rust integration tests, C++ tests, docs, and CI. The final gate must cover representative changed surfaces, the repo-wide quality bar, and the native Windows #36 CI proof.

- [ ] **Step 1: Run focused broker verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-broker --test mcp_assets
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-broker --test multi_target -- --nocapture
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp -- --nocapture
```

Expected: all commands pass.

- [ ] **Step 2: Run focused C++ verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
command -v wine >/dev/null && make -C crates/remote-exec-daemon-cpp test-wine-session-store test-wine-transfer || true
```

Expected: all non-conditional commands pass. If Wine is installed, the Wine targets pass; if Wine is absent, the conditional command exits successfully without running them.

- [ ] **Step 3: Verify native Windows C++ daemon runtime coverage in CI.**

For a local pre-push check, verify the workflow contains the native Windows command:

```bash
rg -n "all-windows-native|windows_cpp_daemon_smoke" .github/workflows/ci.yml
```

Expected: output shows both the host-native C++ daemon build and the Rust broker smoke test.

After pushing the branch, the `cpp (windows-latest)` job must pass this native Windows command:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports_cpp windows_cpp_daemon_smoke -- --nocapture
```

Expected: the GitHub Actions Windows runner starts `remote-exec-daemon-cpp.exe` natively and the smoke test passes.

- [ ] **Step 4: Run no-default-feature verification.**

Run:

```bash
cargo test -p remote-exec-broker --no-default-features --tests
cargo test -p remote-exec-daemon --no-default-features --tests
cargo test -p remote-exec-host --no-default-features --tests
cargo clippy -p remote-exec-broker --no-default-features --all-targets -- -D warnings
cargo clippy -p remote-exec-daemon --no-default-features --all-targets -- -D warnings
cargo clippy -p remote-exec-host --no-default-features --all-targets -- -D warnings
```

Expected: all commands pass.

- [ ] **Step 5: Run full Rust quality gate.**

Run:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all commands pass.

- [ ] **Step 6: Commit any verification fixes.**

If formatting, lint, or test fixes were needed, commit only those fixes:

```bash
git add crates README.md .github/workflows/ci.yml docs/superpowers/plans/2026-05-11-phase-d4-test-reliability.md
git commit -m "chore: satisfy phase d4 quality gate"
```

If Steps 1-5 pass without producing changes, do not create an empty commit.
