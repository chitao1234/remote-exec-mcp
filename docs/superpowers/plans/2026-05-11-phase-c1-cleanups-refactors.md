# Phase C1 Cleanups Refactors Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve Phase C cleanup and refactor items from `docs/CODE_AUDIT_ROUND2.md` without mixing in the operational timeout/startup work reserved for Phase C2.

**Architecture:** Keep C1 as low-behavior-change cleanup: centralize duplicated types/constants, remove ad-hoc routing and logging, split large files behind stable re-exports, and add focused regression tests for subtle contract behavior. Public Rust module paths should remain source-compatible where feasible through re-exports; C++ splits should preserve the existing build targets and POSIX/XP source inventories.

**Tech Stack:** Rust 2024 workspace with Serde/Schemars/Tokio/reqwest/rmcp; C++11 daemon with POSIX and Windows XP-compatible builds; existing Cargo integration tests and C++ make targets.

---

## Scope

Included C1 audit items:

- `#26` large-file decomposition for `remote-exec-proto/src/rpc.rs` and C++ `session_store.cpp`
- `#29` shared HTTP auth config shape
- `#30` shared Rust tunnel queue default
- `#31` typed exec structured logging
- `#32` named exec poll intervals
- `#33` enum-based transfer endpoint routing
- `#34` remove misleading local transfer methods from generic `TargetHandle`
- `#35` direct-tool dispatch registry/check
- `#36` production `expect` / silent local-port cwd fallback cleanup
- `#38` RPC internal-code alias tests
- `#39` C++ session output pump exception logging
- `#40` broker UDP connector sweep critical-section reduction

Deferred to Phase C2:

- `#27` broker-to-daemon per-RPC timeout and reqwest client timeout policy
- `#28` parallel, bounded startup target probes
- `#37` C++ `HTTP_CONNECTION_IDLE_TIMEOUT_MS` config surface

Already addressed before C1:

- `#18` streamable HTTP SSE intervals use `SseInterval`, not `Option<u64>` plus a sentinel.

## Current Validation Snapshot

- `#26` remains valid: `crates/remote-exec-proto/src/rpc.rs` is about 1033 lines and `crates/remote-exec-daemon-cpp/src/session_store.cpp` is about 960 lines.
- `#29` remains valid: broker and daemon still define separate `HttpAuthConfig` structs with duplicated bearer-token validation.
- `#30` remains valid for Rust: `8 * 1024 * 1024` remains in broker tunnel defaults, broker forward limits, broker tests, and host limits.
- `#31` remains valid: `tools/exec.rs` still uses `structured["session_id"]` and `structured["exit_code"]` for completion logging.
- `#32` remains valid: `EXEC_START_POLL_INTERVAL_MS` exists, but `support.rs` and Windows backend smoke checks still use inline poll/timeout literals.
- `#33` remains valid: transfer routing still matches `endpoint.target.as_str()` and `"local"` in several places.
- `#34` remains valid: `TargetHandle` exposes transfer methods that return `unsupported_local_transfer_error()` for local backends.
- `#35` remains valid: `call_direct_tool` manually matches tool-name strings separately from MCP registration.
- `#36` remains valid: `normalize_content` uses `expect`, and `local_port_backend` silently falls back to `temp_dir()` if `current_dir()` fails.
- `#38` remains valid: `RpcErrorCode::from_wire_value` accepts both `"internal"` and `"internal_error"`, but there are no focused tests for alias and unknown-code behavior.
- `#39` remains valid: C++ `pump_session_output` swallows `std::exception` details.
- `#40` remains valid in broker code: `UdpConnectorMap::sweep_idle` scans and removes under one lock.

## File Structure

- `crates/remote-exec-proto/src/auth.rs`: new shared HTTP bearer auth config and validation helpers.
- `crates/remote-exec-proto/src/port_forward.rs`: Rust tunnel queue default constant.
- `crates/remote-exec-proto/src/rpc.rs` and new `crates/remote-exec-proto/src/rpc/*.rs`: split RPC DTOs while preserving `remote_exec_proto::rpc::*` re-exports.
- `crates/remote-exec-broker/src/config.rs`, `daemon_client.rs`, `mcp_server.rs`, `client.rs`, `local_port_backend.rs`, `tools/exec.rs`, `tools/transfer/*.rs`, `target/handle.rs`, `target/mod.rs`, `port_forward/*.rs`: C1 broker cleanup.
- `crates/remote-exec-daemon/src/config/mod.rs`, `http/auth.rs`, `config/tests.rs`: shared HTTP auth config use on the Rust daemon side.
- `crates/remote-exec-host/src/exec/timing.rs`, `exec/handlers.rs`, `exec/support.rs`, `exec/session/windows.rs`: named exec timing constants.
- `crates/remote-exec-daemon-cpp/include/session_pump.h`, `src/session_pump.cpp`, `src/session_store.cpp`, `mk/sources.mk`: C++ session pump split and logging.

---

### Task 1: Save The Phase C1 Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-c1-cleanups-refactors.md`
- Test/Verify: `test -f docs/superpowers/plans/2026-05-11-phase-c1-cleanups-refactors.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked plan artifact only.

- [ ] **Step 1: Verify this plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-c1-cleanups-refactors.md`
Expected: command exits successfully.

- [ ] **Step 2: Review the C1/C2 scope split.**

Run: `sed -n '1,90p' docs/superpowers/plans/2026-05-11-phase-c1-cleanups-refactors.md`
Expected: output includes the required agentic-worker header, includes C1 audit items, and explicitly defers `#27`, `#28`, and `#37` to Phase C2.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-c1-cleanups-refactors.md
git commit -m "docs: plan phase c1 cleanups"
```

### Task 2: Share HTTP Bearer Auth Config

**Finding:** `#29`

**Files:**
- Create: `crates/remote-exec-proto/src/auth.rs`
- Modify: `crates/remote-exec-proto/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Modify: `crates/remote-exec-daemon/src/http/auth.rs`
- Modify: `crates/remote-exec-daemon/src/config/tests.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto auth`
  - `cargo test -p remote-exec-broker config::tests::load_accepts_http_bearer_auth_for_target`
  - `cargo test -p remote-exec-daemon config::tests::load_accepts_http_bearer_auth`
  - `cargo test -p remote-exec-daemon --test health plain_http_bearer_auth`

**Testing approach:** existing tests + targeted unit tests
Reason: This is a shared DTO/validation refactor. Existing broker and daemon config tests protect TOML shape and error messages; proto tests protect the shared helper.

- [ ] **Step 1: Add the shared auth module.**

Create `crates/remote-exec-proto/src/auth.rs`:

```rust
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HttpAuthConfig {
    pub bearer_token: String,
}

impl HttpAuthConfig {
    pub fn validate(&self, scope: &str) -> anyhow::Result<()> {
        let prefix = if scope.is_empty() {
            String::new()
        } else {
            format!("{scope} ")
        };
        anyhow::ensure!(
            !self.bearer_token.is_empty(),
            "{prefix}http_auth.bearer_token must not be empty"
        );
        anyhow::ensure!(
            !self.bearer_token.chars().any(char::is_whitespace),
            "{prefix}http_auth.bearer_token must not contain whitespace"
        );
        Ok(())
    }

    pub fn authorization_header_value(&self) -> String {
        format!("Bearer {}", self.bearer_token)
    }
}

#[cfg(test)]
mod tests {
    use super::HttpAuthConfig;

    #[test]
    fn authorization_header_value_uses_bearer_scheme() {
        let config = HttpAuthConfig {
            bearer_token: "shared-secret".to_string(),
        };
        assert_eq!(config.authorization_header_value(), "Bearer shared-secret");
    }

    #[test]
    fn validate_can_emit_scoped_and_unscoped_messages() {
        let config = HttpAuthConfig {
            bearer_token: String::new(),
        };
        assert_eq!(
            config.validate("").unwrap_err().to_string(),
            "http_auth.bearer_token must not be empty"
        );
        assert_eq!(
            config.validate("target `builder`").unwrap_err().to_string(),
            "target `builder` http_auth.bearer_token must not be empty"
        );
    }
}
```

In `crates/remote-exec-proto/src/lib.rs`, add:

```rust
pub mod auth;
```

- [ ] **Step 2: Use the shared type in broker config and client auth header construction.**

In `crates/remote-exec-broker/src/config.rs`, add:

```rust
pub use remote_exec_proto::auth::HttpAuthConfig;
```

Delete the local `HttpAuthConfig` struct and its `impl`. Keep call sites using `http_auth.validate(&format!("target `{name}`"))?`.

In `crates/remote-exec-broker/src/daemon_client.rs`, change:

```rust
HeaderValue::from_str(&format!("Bearer {}", http_auth.bearer_token))
```

to:

```rust
HeaderValue::from_str(&http_auth.authorization_header_value())
```

- [ ] **Step 3: Use the shared type in daemon config and request auth.**

In `crates/remote-exec-daemon/src/config/mod.rs`, replace the local auth struct with:

```rust
pub use remote_exec_proto::auth::HttpAuthConfig;
```

Delete `prepare_runtime_fields` from `HttpAuthConfig` and make `DaemonConfig::prepare_runtime_fields` a no-op:

```rust
pub fn prepare_runtime_fields(&mut self) {}
```

Change daemon validation to:

```rust
if let Some(http_auth) = &self.http_auth {
    http_auth.validate("")?;
}
```

In `crates/remote-exec-daemon/src/http/auth.rs`, change the comparison to:

```rust
let expected = http_auth.authorization_header_value();
if actual == Some(expected.as_str()) {
    return next.run(request).await;
}
```

In `crates/remote-exec-daemon/src/config/tests.rs`, replace reads of `auth.expected_authorization.as_str()` with:

```rust
auth.authorization_header_value()
```

- [ ] **Step 4: Run focused auth tests.**

Run:

```bash
cargo test -p remote-exec-proto auth
cargo test -p remote-exec-broker config::tests::load_accepts_http_bearer_auth_for_target
cargo test -p remote-exec-daemon config::tests::load_accepts_http_bearer_auth
cargo test -p remote-exec-daemon --test health plain_http_bearer_auth
```

Expected: all tests pass.

- [ ] **Step 5: Confirm duplicate auth structs are gone.**

Run: `rg -n 'struct HttpAuthConfig|expected_authorization|format!\("Bearer' crates/remote-exec-broker/src crates/remote-exec-daemon/src crates/remote-exec-proto/src`
Expected: only the shared `remote-exec-proto/src/auth.rs` struct remains, and there are no `expected_authorization` fields.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-proto/src/auth.rs crates/remote-exec-proto/src/lib.rs crates/remote-exec-broker/src/config.rs crates/remote-exec-broker/src/daemon_client.rs crates/remote-exec-daemon/src/config/mod.rs crates/remote-exec-daemon/src/http/auth.rs crates/remote-exec-daemon/src/config/tests.rs
git commit -m "refactor: share http auth config"
```

### Task 3: Centralize Rust Port Tunnel Queue Default

**Finding:** `#30`

**Files:**
- Modify: `crates/remote-exec-proto/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/limits.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-host/src/config/mod.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto port_forward`
  - `cargo test -p remote-exec-broker port_forward`
  - `cargo test -p remote-exec-host port_forward::port_tunnel_tests::tunnel_ready_reports_configured_limits`

**Testing approach:** existing tests + targeted verification
Reason: This only changes the source of a default value. Existing port-forward tests assert effective limit behavior.

- [ ] **Step 1: Add a shared Rust default constant.**

In `crates/remote-exec-proto/src/port_forward.rs`, add near the top:

```rust
pub const DEFAULT_TUNNEL_QUEUE_BYTES: u64 = 8 * 1024 * 1024;
```

- [ ] **Step 2: Use the shared default in broker tunnel and broker limits.**

In `crates/remote-exec-broker/src/port_forward/tunnel.rs`, import the constant and change:

```rust
pub const DEFAULT_MAX_QUEUED_BYTES: usize = 8 * 1024 * 1024;
```

to:

```rust
pub const DEFAULT_MAX_QUEUED_BYTES: usize =
    remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES as usize;
```

In `crates/remote-exec-broker/src/port_forward/limits.rs`, change the default:

```rust
max_tunnel_queued_bytes: remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES,
```

- [ ] **Step 3: Use the shared default in host config and tests.**

In `crates/remote-exec-host/src/config/mod.rs`, change:

```rust
max_tunnel_queued_bytes: remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES as usize,
```

In broker tests under `port_forward/store.rs`, replace bare `8 * 1024 * 1024` values with:

```rust
remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES
```

or cast to `usize` where the receiving field is `usize`.

- [ ] **Step 4: Run focused tests and search for Rust duplicate defaults.**

Run:

```bash
cargo test -p remote-exec-proto port_forward
cargo test -p remote-exec-broker port_forward
cargo test -p remote-exec-host port_forward::port_tunnel_tests::tunnel_ready_reports_configured_limits
rg -n '8 \* 1024 \* 1024|8388608' crates/remote-exec-broker/src crates/remote-exec-host/src crates/remote-exec-proto/src
```

Expected: tests pass. Search output may include public JSON snapshot values only if they are explicitly asserting serialized output; all Rust default construction should use `DEFAULT_TUNNEL_QUEUE_BYTES`.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-proto/src/port_forward.rs crates/remote-exec-broker/src/port_forward crates/remote-exec-host/src/config/mod.rs
git commit -m "refactor: centralize rust tunnel queue default"
```

### Task 4: Type Exec Tool Completion Logging

**Finding:** `#31`

**Files:**
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker tools::exec`
  - `cargo test -p remote-exec-broker --test mcp_exec`

**Testing approach:** TDD
Reason: This is a small pure conversion around structured output. A unit test can prove the logger path no longer depends on string-key indexing.

- [ ] **Step 1: Make `CommandToolResult` deserializable.**

In `crates/remote-exec-proto/src/public.rs`, change:

```rust
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandToolResult {
```

to:

```rust
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CommandToolResult {
```

- [ ] **Step 2: Add a focused helper test in `tools/exec.rs`.**

Add this to the `#[cfg(test)]` module in `crates/remote-exec-broker/src/tools/exec.rs`, or create one at the end of the file:

```rust
#[cfg(test)]
mod tests {
    use super::command_tool_result_for_logging;

    #[test]
    fn command_tool_result_for_logging_reads_typed_fields() {
        let value = serde_json::json!({
            "target": "local",
            "chunk_id": null,
            "wall_time_seconds": 0.25,
            "exit_code": null,
            "session_id": "session-1",
            "session_command": "sleep 10",
            "original_token_count": null,
            "output": "",
            "warnings": []
        });

        let result = command_tool_result_for_logging(&value).unwrap();
        assert_eq!(result.session_id.as_deref(), Some("session-1"));
        assert_eq!(result.exit_code, None);
    }
}
```

Run: `cargo test -p remote-exec-broker command_tool_result_for_logging`
Expected: compile fails until the helper exists.

- [ ] **Step 3: Replace ad-hoc JSON key access.**

In `crates/remote-exec-broker/src/tools/exec.rs`, add:

```rust
fn command_tool_result_for_logging(
    structured: &serde_json::Value,
) -> Option<remote_exec_proto::public::CommandToolResult> {
    serde_json::from_value(structured.clone()).ok()
}
```

Change the `write_stdin` `.inspect` block to:

```rust
let structured = output
    .structured
    .as_ref()
    .expect("write_stdin tool output should include structured content");
let loggable = command_tool_result_for_logging(structured);
tracing::info!(
    tool = "write_stdin",
    session_id = %session_id,
    requested_target = requested_target.as_deref().unwrap_or("-"),
    running = loggable
        .as_ref()
        .and_then(|result| result.session_id.as_ref())
        .is_some(),
    exit_code = loggable
        .as_ref()
        .and_then(|result| result.exit_code)
        .unwrap_or(-1),
    elapsed_ms = started.elapsed().as_millis() as u64,
    "broker tool completed"
);
```

- [ ] **Step 4: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-broker command_tool_result_for_logging
cargo test -p remote-exec-broker --test mcp_exec
rg -n 'structured\[' crates/remote-exec-broker/src/tools/exec.rs
```

Expected: tests pass and the search has no output.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-proto/src/public.rs crates/remote-exec-broker/src/tools/exec.rs
git commit -m "refactor: type exec tool logging fields"
```

### Task 5: Name Shared Exec Poll Timings

**Finding:** `#32`

**Files:**
- Create: `crates/remote-exec-host/src/exec/timing.rs`
- Modify: `crates/remote-exec-host/src/exec/mod.rs`
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/exec/session/windows.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host exec::support`
  - `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** existing tests + targeted verification
Reason: This is a naming/refactor change around existing timing values. Exec RPC tests exercise the polling paths.

- [ ] **Step 1: Add shared timing constants.**

Create `crates/remote-exec-host/src/exec/timing.rs`:

```rust
use std::time::Duration;

pub(super) const EXEC_POLL_INTERVAL: Duration = Duration::from_millis(25);
pub(super) const WINDOWS_BACKEND_SMOKE_TIMEOUT: Duration = Duration::from_millis(300);
```

In `crates/remote-exec-host/src/exec/mod.rs`, add:

```rust
mod timing;
```

- [ ] **Step 2: Replace exec poll literals.**

In `handlers.rs`, remove `EXEC_START_POLL_INTERVAL_MS` and change:

```rust
tokio::time::sleep(Duration::from_millis(EXEC_START_POLL_INTERVAL_MS)).await;
```

to:

```rust
tokio::time::sleep(super::timing::EXEC_POLL_INTERVAL).await;
```

In `support.rs`, change:

```rust
tokio::time::sleep(Duration::from_millis(25)).await;
```

to:

```rust
tokio::time::sleep(super::timing::EXEC_POLL_INTERVAL).await;
```

In `session/windows.rs`, change the smoke-test deadline and sleep to:

```rust
let deadline = Instant::now() + super::super::timing::WINDOWS_BACKEND_SMOKE_TIMEOUT;
tokio::time::sleep(super::super::timing::EXEC_POLL_INTERVAL).await;
```

and format the timeout message from the constant:

```rust
let timeout_ms = super::super::timing::WINDOWS_BACKEND_SMOKE_TIMEOUT.as_millis();
format!(
    "{} smoke test: still running after {timeout_ms}ms; output={}",
    backend.debug_name(),
    summarize_output_excerpt(&output)
)
```

- [ ] **Step 3: Run focused tests and search.**

Run:

```bash
cargo test -p remote-exec-host exec::support
cargo test -p remote-exec-daemon --test exec_rpc
rg -n 'Duration::from_millis\(25\)|Duration::from_millis\(300\)|EXEC_START_POLL_INTERVAL_MS' crates/remote-exec-host/src/exec
```

Expected: tests pass. Search output should not include unnamed production poll literals in `handlers.rs`, `support.rs`, or `session/windows.rs`.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-host/src/exec/timing.rs crates/remote-exec-host/src/exec/mod.rs crates/remote-exec-host/src/exec/handlers.rs crates/remote-exec-host/src/exec/support.rs crates/remote-exec-host/src/exec/session/windows.rs
git commit -m "refactor: name exec poll timings"
```

### Task 6: Introduce Typed Transfer Endpoint Routing

**Finding:** `#33`

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_transfer`

**Testing approach:** existing tests + targeted verification
Reason: This is a routing refactor. The broker transfer integration tests cover local/local, local/remote, remote/local, and remote/remote behavior.

- [ ] **Step 1: Add an endpoint route enum.**

In `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TransferEndpointTarget<'a> {
    Local,
    Remote(&'a str),
}

impl<'a> TransferEndpointTarget<'a> {
    pub(super) fn from_name(target_name: &'a str) -> Self {
        if target_name == "local" {
            Self::Local
        } else {
            Self::Remote(target_name)
        }
    }

    pub(super) fn from_endpoint(endpoint: &'a TransferEndpoint) -> Self {
        Self::from_name(endpoint.target.as_str())
    }
}
```

Change `endpoint_target_context` to use `TransferEndpointTarget`:

```rust
async fn endpoint_target_context(
    state: &crate::BrokerState,
    target_name: &str,
) -> anyhow::Result<EndpointTargetContext> {
    match TransferEndpointTarget::from_name(target_name) {
        TransferEndpointTarget::Local => Ok(EndpointTargetContext::local()),
        TransferEndpointTarget::Remote(target_name) => Ok(EndpointTargetContext::remote(
            verified_remote_daemon_info(state, target_name).await?,
        )),
    }
}
```

- [ ] **Step 2: Replace transfer operation string matches.**

In `operations.rs`, import:

```rust
use super::endpoints::{TransferEndpointTarget, endpoint_policy, verified_remote_target};
```

Change `transfer_single_source` to match on typed endpoints:

```rust
pub(super) async fn transfer_single_source(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
    options: TransferExecutionOptions<'_>,
) -> anyhow::Result<(TransferSourceType, TransferImportResponse)> {
    match (
        TransferEndpointTarget::from_endpoint(source),
        TransferEndpointTarget::from_endpoint(destination),
    ) {
        (TransferEndpointTarget::Local, TransferEndpointTarget::Local) => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let exported = crate::local_transfer::export_path_to_stream(
                &source.path,
                &export_request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            let request = build_import_request(
                destination,
                options.overwrite,
                exported.source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let summary = crate::local_transfer::import_archive_from_async_reader(
                exported.reader,
                &request,
                state.host_sandbox.as_ref(),
                state.transfer_limits,
            )
            .await?;
            Ok((exported.source_type, summary))
        }
        (TransferEndpointTarget::Local, TransferEndpointTarget::Remote(target_name)) => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let exported = crate::local_transfer::export_path_to_stream(
                &source.path,
                &export_request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            let request = build_import_request(
                destination,
                options.overwrite,
                exported.source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let body =
                reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(exported.reader));
            let summary =
                import_remote_body_to_endpoint(state, target_name, body, &request).await?;
            Ok((exported.source_type, summary))
        }
        (TransferEndpointTarget::Remote(target_name), TransferEndpointTarget::Local) => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let target = verified_remote_target(state, target_name).await?;
            let exported = handle_remote_transfer_result(
                target,
                target.transfer_export_stream(&export_request).await,
            )
            .await?;
            let source_type = exported.source_type.clone();
            let request = build_import_request(
                destination,
                options.overwrite,
                source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let summary = crate::local_transfer::import_archive_from_async_reader(
                exported.into_async_read(),
                &request,
                state.host_sandbox.as_ref(),
                state.transfer_limits,
            )
            .await?;
            Ok((source_type, summary))
        }
        (
            TransferEndpointTarget::Remote(source_target_name),
            TransferEndpointTarget::Remote(destination_target_name),
        ) => {
            let export_request = build_export_request(
                source,
                options.compression,
                options.exclude,
                options.symlink_mode,
            );
            let source_target = verified_remote_target(state, source_target_name).await?;
            let exported = handle_remote_transfer_result(
                source_target,
                source_target.transfer_export_stream(&export_request).await,
            )
            .await?;
            let source_type = exported.source_type.clone();
            let request = build_import_request(
                destination,
                options.overwrite,
                source_type.clone(),
                options.compression,
                options.symlink_mode,
                options.create_parent,
            );
            let summary = import_remote_body_to_endpoint(
                state,
                destination_target_name,
                exported.into_body(),
                &request,
            )
            .await?;
            Ok((source_type, summary))
        }
    }
}
```

Change `export_endpoint_to_archive` to match `TransferEndpointTarget::from_endpoint(endpoint)`:

```rust
match TransferEndpointTarget::from_endpoint(endpoint) {
    TransferEndpointTarget::Local => {
        let exported = crate::local_transfer::export_path_to_archive(
            &endpoint.path,
            archive_path,
            &request,
            state.host_sandbox.as_ref(),
        )
        .await?;
        Ok(ExportArchiveResult {
            source_type: exported.source_type,
        })
    }
    TransferEndpointTarget::Remote(target_name) => {
        export_remote_endpoint_to_archive(state, target_name, &request, archive_path).await
    }
}
```

Change `import_archive_to_endpoint` to match `TransferEndpointTarget::from_endpoint(endpoint)`:

```rust
match TransferEndpointTarget::from_endpoint(endpoint) {
    TransferEndpointTarget::Local => {
        crate::local_transfer::import_archive_from_file(
            archive_path,
            request,
            state.host_sandbox.as_ref(),
            state.transfer_limits,
        )
        .await
    }
    TransferEndpointTarget::Remote(target_name) => {
        import_remote_archive_to_endpoint(state, target_name, archive_path, request).await
    }
}
```

- [ ] **Step 3: Replace endpoint helper string matches.**

In `endpoints.rs`, change `existing_destination_is_directory` to match `TransferEndpointTarget::from_endpoint(destination)`:

```rust
let result = match TransferEndpointTarget::from_endpoint(destination) {
    TransferEndpointTarget::Local => {
        crate::local_transfer::path_info(&destination.path, state.host_sandbox.as_ref())
    }
    TransferEndpointTarget::Remote(target_name) => {
        let target = verified_remote_target(state, target_name).await?;
        target
            .clear_on_transport_error(
                target
                    .transfer_path_info(&TransferPathInfoRequest {
                        path: destination.path.clone(),
                    })
                    .await,
            )
            .await
    }
};
```

- [ ] **Step 4: Run transfer tests and search.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_transfer
rg -n 'target\.as_str\(\)|match endpoint\.target|match destination\.target|match source\.target' crates/remote-exec-broker/src/tools/transfer
rg -n '== "local"' crates/remote-exec-broker/src/tools/transfer
```

Expected: tests pass. The first search has no output. The second search may show only the centralized `TransferEndpointTarget::from_name` comparison.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-broker/src/tools/transfer/endpoints.rs crates/remote-exec-broker/src/tools/transfer/operations.rs
git commit -m "refactor: type transfer endpoint routing"
```

### Task 7: Remove Local Transfer Traps From TargetHandle

**Finding:** `#34`

**Files:**
- Modify: `crates/remote-exec-broker/src/target/handle.rs`
- Modify: `crates/remote-exec-broker/src/target/mod.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_transfer`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** characterization/integration test
Reason: This is a type-safety refactor. The transfer tests prove behavior stays stable, and forward-port tests cover local target handle behavior that must remain available for tunnels.

- [ ] **Step 1: Add a remote-only target wrapper.**

In `crates/remote-exec-broker/src/target/handle.rs`, add near `TargetHandle`:

```rust
#[derive(Clone, Copy)]
pub(crate) struct RemoteTargetHandle<'a> {
    handle: &'a TargetHandle,
    client: &'a crate::daemon_client::DaemonClient,
}
```

Add this method to `impl TargetHandle`:

```rust
pub(crate) fn as_remote(&self) -> Option<RemoteTargetHandle<'_>> {
    match &self.backend {
        TargetBackend::Remote(client) => Some(RemoteTargetHandle {
            handle: self,
            client,
        }),
        TargetBackend::Local(_) => None,
    }
}
```

Add `impl RemoteTargetHandle<'_>` with the remote-only transfer methods:

```rust
impl RemoteTargetHandle<'_> {
    pub async fn cached_daemon_info(&self) -> Option<CachedDaemonInfo> {
        self.handle.cached_daemon_info().await
    }

    pub async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferExportResponse, DaemonClientError> {
        self.client.transfer_export_to_file(req, archive_path).await
    }

    pub async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        self.client.transfer_export_stream(req).await
    }

    pub async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        self.client.transfer_path_info(req).await
    }

    pub async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        self.client.transfer_import_from_file(archive_path, req).await
    }

    pub async fn transfer_import_from_body(
        &self,
        req: &TransferImportRequest,
        body: reqwest::Body,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        self.client.transfer_import_from_body(req, body).await
    }

    pub async fn clear_on_transport_error<T>(
        &self,
        result: Result<T, DaemonClientError>,
    ) -> Result<T, DaemonClientError> {
        self.handle.clear_on_transport_error(result).await
    }
}
```

Keep `TransferPathInfoRequest` and `TransferPathInfoResponse` in the existing `use remote_exec_proto::rpc::{...}` list in `handle.rs`; `TargetHandle` still serves path-info for local and remote backends, and `RemoteTargetHandle` forwards the remote-only call.

- [ ] **Step 2: Remove generic transfer trap methods.**

Delete these methods from `TargetHandle`:

```rust
transfer_export_to_file
transfer_export_stream
transfer_import_from_file
transfer_import_from_body
```

Delete `unsupported_local_transfer_error()`.

In `crates/remote-exec-broker/src/target/mod.rs`, export the wrapper:

```rust
pub(crate) use handle::RemoteTargetHandle;
```

- [ ] **Step 3: Make `verified_remote_target` return the wrapper.**

In `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`, change:

```rust
pub(super) async fn verified_remote_target<'a>(
    state: &'a crate::BrokerState,
    target_name: &'a str,
) -> anyhow::Result<&'a crate::TargetHandle> {
```

to:

```rust
pub(super) async fn verified_remote_target<'a>(
    state: &'a crate::BrokerState,
    target_name: &'a str,
) -> anyhow::Result<crate::target::RemoteTargetHandle<'a>> {
```

and return:

```rust
target
    .as_remote()
    .with_context(|| format!("target `{target_name}` is not a remote transfer target"))
```

after `target.ensure_identity_verified(target_name).await?`.

- [ ] **Step 4: Update transfer operation call sites.**

No behavior should change in `operations.rs`; calls such as:

```rust
target.transfer_export_stream(&export_request).await
```

should now resolve on `RemoteTargetHandle`.

Change `handle_remote_transfer_result` in `operations.rs` to take the wrapper and delegate transport cleanup through it:

```rust
async fn handle_remote_transfer_result<T>(
    target: crate::target::RemoteTargetHandle<'_>,
    result: Result<T, DaemonClientError>,
) -> anyhow::Result<T> {
    match target.clear_on_transport_error(result).await {
        Ok(value) => Ok(value),
        Err(err) => Err(normalize_transfer_error(err)),
    }
}
```

- [ ] **Step 5: Run tests and search.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_transfer
cargo test -p remote-exec-broker --test mcp_forward_ports
rg -n 'unsupported_local_transfer_error|transfer_export_stream\(|transfer_import_from_body\(' crates/remote-exec-broker/src/target crates/remote-exec-broker/src/tools/transfer
```

Expected: tests pass. Search should not show unsupported local transfer traps in `TargetHandle`.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/target/handle.rs crates/remote-exec-broker/src/target/mod.rs crates/remote-exec-broker/src/tools/transfer/endpoints.rs crates/remote-exec-broker/src/tools/transfer/operations.rs
git commit -m "refactor: split remote transfer handle"
```

### Task 8: Add A Shared Broker Tool Registry Check

**Finding:** `#35`

**Files:**
- Create: `crates/remote-exec-broker/src/tools/registry.rs`
- Modify: `crates/remote-exec-broker/src/tools/mod.rs`
- Modify: `crates/remote-exec-broker/src/client.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker tools::registry`
  - `cargo test -p remote-exec-broker --test mcp_assets list_targets_is_advertised_as_read_only`

**Testing approach:** focused unit test + existing MCP advertising test
Reason: The `rmcp` tool macro still requires literal names at registration sites, so C1 should at least centralize direct dispatch and add a test that catches drift between the registry and the registered public tool names.

- [ ] **Step 1: Add the broker tool registry.**

Create `crates/remote-exec-broker/src/tools/registry.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrokerTool {
    ListTargets,
    ExecCommand,
    WriteStdin,
    ApplyPatch,
    ViewImage,
    TransferFiles,
    ForwardPorts,
}

impl BrokerTool {
    pub(crate) const NAMES: &'static [&'static str] = &[
        "list_targets",
        "exec_command",
        "write_stdin",
        "apply_patch",
        "view_image",
        "transfer_files",
        "forward_ports",
    ];

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "list_targets" => Some(Self::ListTargets),
            "exec_command" => Some(Self::ExecCommand),
            "write_stdin" => Some(Self::WriteStdin),
            "apply_patch" => Some(Self::ApplyPatch),
            "view_image" => Some(Self::ViewImage),
            "transfer_files" => Some(Self::TransferFiles),
            "forward_ports" => Some(Self::ForwardPorts),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ListTargets => "list_targets",
            Self::ExecCommand => "exec_command",
            Self::WriteStdin => "write_stdin",
            Self::ApplyPatch => "apply_patch",
            Self::ViewImage => "view_image",
            Self::TransferFiles => "transfer_files",
            Self::ForwardPorts => "forward_ports",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BrokerTool;

    #[test]
    fn all_tool_names_round_trip_through_registry() {
        for name in BrokerTool::NAMES {
            let tool = BrokerTool::from_name(name).expect("registered tool should parse");
            assert_eq!(tool.name(), *name);
        }
    }
}
```

In `crates/remote-exec-broker/src/tools/mod.rs`, add:

```rust
pub(crate) mod registry;
```

- [ ] **Step 2: Use the registry in direct client dispatch.**

In `crates/remote-exec-broker/src/client.rs`, import:

```rust
use crate::tools::registry::BrokerTool;
```

Change the direct dispatch match to use `BrokerTool`:

```rust
match BrokerTool::from_name(name) {
    Some(BrokerTool::ListTargets) => {
        invoke_tool!(ListTargetsInput, crate::tools::targets::list_targets)
    }
    Some(BrokerTool::ExecCommand) => {
        invoke_tool!(ExecCommandInput, crate::tools::exec::exec_command)
    }
    Some(BrokerTool::WriteStdin) => {
        invoke_tool!(WriteStdinInput, crate::tools::exec::write_stdin)
    }
    Some(BrokerTool::ApplyPatch) => {
        invoke_tool!(ApplyPatchInput, crate::tools::patch::apply_patch)
    }
    Some(BrokerTool::ViewImage) => {
        invoke_tool!(ViewImageInput, crate::tools::image::view_image)
    }
    Some(BrokerTool::TransferFiles) => {
        invoke_tool!(TransferFilesInput, crate::tools::transfer::transfer_files)
    }
    Some(BrokerTool::ForwardPorts) => {
        invoke_tool!(ForwardPortsInput, crate::tools::port_forward::forward_ports)
    }
    None => crate::mcp_server::tool_error_result(format!("unknown tool `{name}`")),
}
```

- [ ] **Step 3: Add an MCP registration drift test.**

In `registry.rs`, add a test-only list mirroring the `#[tool]` literal names:

```rust
#[cfg(test)]
mod mcp_registration_contract_tests {
    use super::BrokerTool;

    const MCP_TOOL_NAMES: &[&str] = &[
        "list_targets",
        "exec_command",
        "write_stdin",
        "apply_patch",
        "view_image",
        "transfer_files",
        "forward_ports",
    ];

    #[test]
    fn registry_matches_mcp_tool_names() {
        assert_eq!(BrokerTool::NAMES, MCP_TOOL_NAMES);
    }
}
```

This is intentionally duplicated in test code because the `rmcp` registration macro consumes string literals. The test makes drift visible.

- [ ] **Step 4: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-broker tools::registry
cargo test -p remote-exec-broker --test mcp_assets list_targets_is_advertised_as_read_only
```

Expected: tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-broker/src/tools/registry.rs crates/remote-exec-broker/src/tools/mod.rs crates/remote-exec-broker/src/client.rs
git commit -m "refactor: centralize direct tool dispatch names"
```

### Task 9: Remove Production Panic And Silent Cwd Fallback

**Finding:** `#36`

**Files:**
- Modify: `crates/remote-exec-broker/src/client.rs`
- Modify: `crates/remote-exec-broker/src/local_port_backend.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_assets view_image_returns_input_image_content_and_structured_content`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_opens_lists_and_closes_local_tcp_forward`

**Testing approach:** existing integration tests + targeted verification
Reason: The serialization fallback is difficult to force with current `rmcp::Content`, and the cwd path is environment-sensitive. Existing tests verify normal behavior remains intact while production no longer panics or silently relocates.

- [ ] **Step 1: Replace raw content serialization `expect`.**

In `crates/remote-exec-broker/src/client.rs`, replace:

```rust
serde_json::to_value(content).expect("serializing raw MCP content")
```

with:

```rust
serde_json::to_value(content).unwrap_or_else(|err| {
    tracing::warn!(error = %err, "failed to serialize raw MCP content");
    serde_json::json!({
        "type": "unsupported_content",
        "error": err.to_string(),
    })
})
```

- [ ] **Step 2: Return cwd errors from local port runtime construction.**

In `crates/remote-exec-broker/src/local_port_backend.rs`, import:

```rust
use anyhow::Context;
```

Change:

```rust
default_workdir: std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()),
```

to:

```rust
default_workdir: std::env::current_dir()
    .context("resolving current directory for local port runtime")?,
```

- [ ] **Step 3: Run focused tests and search.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_assets view_image_returns_input_image_content_and_structured_content
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_opens_lists_and_closes_local_tcp_forward
rg -n 'expect\("serializing raw MCP content"\)|current_dir\(\)\.unwrap_or_else' crates/remote-exec-broker/src
```

Expected: tests pass and search has no output.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-broker/src/client.rs crates/remote-exec-broker/src/local_port_backend.rs
git commit -m "fix: avoid broker production panic paths"
```

### Task 10: Add RPC Error-Code Contract Tests

**Finding:** `#38`

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto rpc_error_code`

**Testing approach:** TDD
Reason: The behavior is pure enum wire conversion. Unit tests are enough and should be preserved when `rpc.rs` is split.

- [ ] **Step 1: Add focused tests.**

In `crates/remote-exec-proto/src/rpc.rs`, inside `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn rpc_error_code_internal_wire_value_round_trips() {
    assert_eq!(RpcErrorCode::Internal.wire_value(), "internal_error");
    assert_eq!(
        RpcErrorCode::from_wire_value("internal_error"),
        Some(RpcErrorCode::Internal)
    );
}

#[test]
fn rpc_error_code_accepts_legacy_internal_alias() {
    assert_eq!(
        RpcErrorCode::from_wire_value("internal"),
        Some(RpcErrorCode::Internal)
    );
}

#[test]
fn rpc_error_code_unknown_wire_value_returns_none() {
    assert_eq!(RpcErrorCode::from_wire_value("future_error_code"), None);
}
```

- [ ] **Step 2: Run the focused tests.**

Run: `cargo test -p remote-exec-proto rpc_error_code`
Expected: tests pass.

- [ ] **Step 3: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc.rs
git commit -m "test: cover rpc error code wire aliases"
```

### Task 11: Shrink UDP Connector Sweep Critical Section

**Finding:** `#40`

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/udp_connectors.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker port_forward::udp_connectors`
  - `cargo test -p remote-exec-broker port_forward::udp_bridge`

**Testing approach:** existing tests + focused unit coverage
Reason: The public behavior should not change. Existing connector and UDP bridge tests cover map consistency and stream cleanup.

- [ ] **Step 1: Split snapshot and removal phases.**

Change `UdpConnectorMap::sweep_idle` to:

```rust
pub(super) async fn sweep_idle(
    &self,
    now: Instant,
    idle_timeout: Duration,
) -> Vec<(u32, UdpPeerConnector)> {
    let expired = {
        let state = self.inner.lock().await;
        state
            .connector_by_peer
            .iter()
            .filter_map(|(peer, connector)| {
                (now.duration_since(connector.last_used) >= idle_timeout)
                    .then_some((peer.clone(), connector.stream_id))
            })
            .collect::<Vec<_>>()
    };

    let mut state = self.inner.lock().await;
    let mut removed = Vec::with_capacity(expired.len());
    for (peer, stream_id) in expired {
        let still_expired = state
            .connector_by_peer
            .get(&peer)
            .is_some_and(|connector| {
                connector.stream_id == stream_id
                    && now.duration_since(connector.last_used) >= idle_timeout
            });
        if !still_expired {
            continue;
        }
        if let Some(connector) = state.connector_by_peer.remove(&peer) {
            state.peer_by_connector.remove(&stream_id);
            removed.push((stream_id, connector));
        }
    }
    removed
}
```

- [ ] **Step 2: Add a test for refreshed entries.**

In `udp_connectors.rs` tests, add:

```rust
#[tokio::test]
async fn connector_map_sweep_keeps_recently_refreshed_connectors() {
    let map = UdpConnectorMap::default();
    let old = Instant::now() - Duration::from_secs(120);
    let fresh = Instant::now();

    map.insert(
        "127.0.0.1:10000".to_string(),
        7,
        UdpPeerConnector {
            stream_id: 7,
            last_used: old,
        },
    )
    .await;
    map.get_mut_by_peer("127.0.0.1:10000", |connector| {
        connector.last_used = fresh;
    })
    .await;

    let removed = map.sweep_idle(fresh, Duration::from_secs(60)).await;
    assert!(removed.is_empty());
    assert_eq!(map.len().await, 1);
}
```

- [ ] **Step 3: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-broker port_forward::udp_connectors
cargo test -p remote-exec-broker port_forward::udp_bridge
```

Expected: tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/udp_connectors.rs
git commit -m "refactor: shorten udp connector sweep lock"
```

### Task 12: Log C++ Session Pump Exceptions

**Finding:** `#39`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-session-store`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** existing tests + targeted build verification
Reason: This adds diagnostics to an existing failure path without changing session state transitions.

- [ ] **Step 1: Log exception details before retiring the session.**

In `pump_session_output`, change:

```cpp
} catch (const std::exception&) {
```

to:

```cpp
} catch (const std::exception& ex) {
    log_message(
        LOG_WARN,
        "session",
        std::string("session output pump failed: ") + ex.what()
    );
```

Keep the existing lock, carry save, `finish_session_output_locked`, and `retired = true` logic after the new log call.

- [ ] **Step 2: Run focused C++ tests.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected: both pass.

- [ ] **Step 3: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/session_store.cpp
git commit -m "fix: log cpp session pump failures"
```

### Task 13: Split RPC Error Types Into A Submodule

**Finding:** `#26`

**Files:**
- Create: `crates/remote-exec-proto/src/rpc/error.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto rpc_error_code`
  - `cargo test -p remote-exec-broker daemon_client::tests::rpc_error_code`

**Testing approach:** existing tests + focused unit tests
Reason: Start the `rpc.rs` split with isolated error DTOs. Re-exports keep downstream imports stable.

- [ ] **Step 1: Create the error submodule.**

Create `crates/remote-exec-proto/src/rpc/error.rs` containing:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorCode {
    BadRequest,
    Unauthorized,
    UnknownSession,
    NotFound,
    UnknownEndpoint,
    InvalidPortTunnel,
    PortTunnelUnavailable,
    PortTunnelLimitExceeded,
    PortTunnelAlreadyAttached,
    PortTunnelResumeExpired,
    PortTunnelGenerationMismatch,
    UnknownPortTunnelSession,
    PortTunnelClosed,
    PortForwardBackpressureExceeded,
    InvalidPortTunnelMetadata,
    InvalidEndpoint,
    PortBindFailed,
    PortAcceptFailed,
    PortConnectFailed,
    PortReadFailed,
    PortWriteFailed,
    PortConnectionClosed,
    UnknownPortConnection,
    UnknownPortBind,
    SandboxDenied,
    StdinClosed,
    TtyDisabled,
    TtyUnsupported,
    LoginShellUnsupported,
    LoginShellDisabled,
    InvalidDetail,
    ImageMissing,
    ImageNotFile,
    ImageDecodeFailed,
    TransferPathNotAbsolute,
    TransferDestinationExists,
    TransferParentMissing,
    TransferDestinationUnsupported,
    TransferCompressionUnsupported,
    TransferSourceUnsupported,
    TransferSourceMissing,
    TransferFailed,
    PatchFailed,
    Internal,
}
```

Move the current `impl RpcErrorCode` and `impl RpcErrorBody` blocks from `rpc.rs` into `rpc/error.rs`, plus the `rpc_error_code_*` tests added in Task 10.

At the top of `rpc.rs`, add:

```rust
mod error;

pub use error::{RpcErrorBody, RpcErrorCode};
```

- [ ] **Step 2: Remove the moved definitions from `rpc.rs`.**

Delete the original `RpcErrorBody`, `RpcErrorCode`, `impl RpcErrorCode`, and `impl RpcErrorBody` blocks from `rpc.rs`.

Update any remaining test imports to use the re-export if needed:

```rust
use super::RpcErrorCode;
```

- [ ] **Step 3: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-proto rpc_error_code
cargo test -p remote-exec-broker daemon_client::tests::rpc_error_code
```

Expected: tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/rpc/error.rs
git commit -m "refactor: split rpc error types"
```

### Task 14: Split RPC Target And Exec DTOs Into Submodules

**Finding:** `#26`

**Files:**
- Create: `crates/remote-exec-proto/src/rpc/target.rs`
- Create: `crates/remote-exec-proto/src/rpc/exec.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto exec_response`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** characterization/integration test
Reason: The split must preserve wire shape for target-info and exec responses. Existing exec tests cover the custom serde invariants.

- [ ] **Step 1: Move target DTOs.**

Create `crates/remote-exec-proto/src/rpc/target.rs` containing:

```rust
use std::num::NonZeroU32;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HealthCheckResponse {
    pub status: String,
    pub daemon_version: String,
    pub daemon_instance_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TargetInfoResponse {
    pub target: String,
    pub daemon_version: String,
    pub daemon_instance_id: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_image_read: bool,
    #[serde(default)]
    pub supports_transfer_compression: bool,
    #[serde(default)]
    pub supports_port_forward: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port_forward_protocol_version: Option<PortForwardProtocolVersion>,
}

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(transparent)]
pub struct PortForwardProtocolVersion(NonZeroU32);

impl PortForwardProtocolVersion {
    pub fn v4() -> Self {
        Self(NonZeroU32::new(4).expect("v4 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}
```

In `rpc.rs`, add:

```rust
mod target;

pub use target::{HealthCheckResponse, PortForwardProtocolVersion, TargetInfoResponse};
```

Delete the moved target DTOs from `rpc.rs`.

- [ ] **Step 2: Move exec DTOs.**

Create `crates/remote-exec-proto/src/rpc/exec.rs` containing the current `ExecStartRequest`, `ExecWriteRequest`, `ExecOutputResponse`, `ExecRunningResponse`, `ExecCompletedResponse`, `ExecResponse`, `ExecResponseWire`, `ExecStartResponse`, `ExecWriteResponse`, `ExecWarning`, `WarningCode`, and their impl blocks. Include necessary imports:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
```

In `rpc.rs`, add:

```rust
mod exec;

pub use exec::{
    ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecRunningResponse,
    ExecStartRequest, ExecStartResponse, ExecWarning, ExecWriteRequest, ExecWriteResponse,
    WarningCode,
};
```

Delete the moved exec DTOs from `rpc.rs`.

- [ ] **Step 3: Move exec response tests beside exec DTOs.**

Move these tests from `rpc.rs` into `rpc/exec.rs`:

```rust
running_exec_response_requires_daemon_session_id
completed_exec_response_rejects_daemon_session_id
completed_exec_response_omits_daemon_session_id
```

- [ ] **Step 4: Run focused exec tests.**

Run:

```bash
cargo test -p remote-exec-proto exec_response
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-daemon --test exec_rpc
```

Expected: tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/rpc/target.rs crates/remote-exec-proto/src/rpc/exec.rs
git commit -m "refactor: split rpc target and exec dto"
```

### Task 15: Split RPC Transfer, Patch, And Image DTOs Into Submodules

**Finding:** `#26`

**Files:**
- Create: `crates/remote-exec-proto/src/rpc/transfer.rs`
- Create: `crates/remote-exec-proto/src/rpc/patch.rs`
- Create: `crates/remote-exec-proto/src/rpc/image.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto transfer_header`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`
  - `cargo test -p remote-exec-daemon --test patch_rpc`
  - `cargo test -p remote-exec-daemon --test image_rpc`
  - `cargo test -p remote-exec-broker --test mcp_transfer`

**Testing approach:** characterization/integration test
Reason: This completes the `rpc.rs` decomposition while preserving the public re-export surface.

- [ ] **Step 1: Move transfer header DTOs and helpers.**

Create `crates/remote-exec-proto/src/rpc/transfer.rs` containing the current transfer header constants, `TransferWarning`, transfer metadata structs, `TransferHeaderErrorKind`, `TransferHeaderError`, `TransferHeaderPairs`, `TransferPathInfoRequest`, `TransferPathInfoResponse`, `TransferImportResponse`, transfer header pair builders, parsers, and transfer-header tests.

Use imports:

```rust
use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::transfer::{
    TransferCompression, TransferExportMetadata, TransferExportRequest, TransferImportMetadata,
    TransferImportRequest, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};
```

In `rpc.rs`, add:

```rust
mod transfer;

pub use transfer::{
    TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TRANSFER_SYMLINK_MODE_HEADER, TransferExportMetadata, TransferHeaderError,
    TransferExportRequest, TransferHeaderErrorKind, TransferHeaderPairs, TransferImportMetadata,
    TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse, TransferWarning,
    parse_transfer_export_metadata, parse_transfer_import_metadata,
    transfer_export_header_pairs, transfer_import_header_pairs,
};
```

Also preserve the current public enum re-exports at the top of `rpc.rs`:

```rust
pub use crate::transfer::{
    TransferCompression, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};
```

- [ ] **Step 2: Move patch DTOs.**

Create `crates/remote-exec-proto/src/rpc/patch.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyRequest {
    pub patch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyResponse {
    pub output: String,
}
```

In `rpc.rs`, add:

```rust
mod patch;

pub use patch::{PatchApplyRequest, PatchApplyResponse};
```

- [ ] **Step 3: Move image and empty response DTOs.**

Create `crates/remote-exec-proto/src/rpc/image.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadRequest {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadResponse {
    pub image_url: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmptyResponse {}
```

In `rpc.rs`, add:

```rust
mod image;

pub use image::{EmptyResponse, ImageReadRequest, ImageReadResponse};
```

- [ ] **Step 4: Remove moved code from `rpc.rs` and run tests.**

Run:

```bash
cargo test -p remote-exec-proto transfer_header
cargo test -p remote-exec-daemon --test transfer_rpc
cargo test -p remote-exec-daemon --test patch_rpc
cargo test -p remote-exec-daemon --test image_rpc
cargo test -p remote-exec-broker --test mcp_transfer
wc -l crates/remote-exec-proto/src/rpc.rs
```

Expected: tests pass. `rpc.rs` should now be a small re-export module rather than a 1000-line mixed DTO file.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/rpc/transfer.rs crates/remote-exec-proto/src/rpc/patch.rs crates/remote-exec-proto/src/rpc/image.rs
git commit -m "refactor: split rpc transfer patch image dto"
```

### Task 16: Extract C++ Session Pump Helpers

**Finding:** `#26`

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/session_pump.h`
- Create: `crates/remote-exec-daemon-cpp/src/session_pump.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-session-store`
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** existing tests + targeted build verification
Reason: This is a C++ file split. Existing session-store tests cover pump/drain/session lifecycle behavior across POSIX, and XP build verifies the Win32 path still compiles.

- [ ] **Step 1: Add the pump header.**

Create `crates/remote-exec-daemon-cpp/include/session_pump.h`:

```cpp
#pragma once

#include <memory>
#include <string>

#include "session_store.h"

bool mark_session_exit_locked(LiveSession* session);
void finish_session_output_locked(LiveSession* session);
void start_session_pump(const std::shared_ptr<LiveSession>& session);
void join_session_pump(LiveSession* session);
std::string take_session_output_locked(LiveSession* session, unsigned long max_output_tokens);
bool drain_exited_session_output_locked(
    LiveSession* session,
    std::string* output,
    unsigned long max_output_tokens
);
```

- [ ] **Step 2: Move pump and output-drain helpers into `session_pump.cpp`.**

Create `crates/remote-exec-daemon-cpp/src/session_pump.cpp` and move these current definitions from `session_store.cpp` into it:

```cpp
append_session_output_locked
mark_session_exit_locked
finish_session_output_locked
terminate_descendants_after_exit_locked
pump_session_output
SessionPumpContext
session_output_pump_entry
start_session_pump
join_session_pump
take_session_output_locked
drain_exited_session_output_locked
```

Also move the helper constants they require:

```cpp
const unsigned long EXIT_DRAIN_INITIAL_WAIT_MS = 125UL;
const unsigned long EXIT_DRAIN_QUIET_MS = 25UL;
```

Leave `EXIT_POLL_INTERVAL_MS`, `wait_for_session_activity`, and output rendering helpers such as `BYTES_PER_TOKEN`, `approximate_token_count`, and `render_output` in `session_store.cpp`; they are used by the store polling and response-building paths, not by the pump thread or drain helper.

Include the dependencies needed by the moved code:

```cpp
#include <algorithm>
#include <sstream>
#include <string>
#include <vector>

#include "logging.h"
#include "platform.h"
#include "process_session.h"
#include "session_pump.h"
#ifdef _WIN32
#include "win32_thread.h"
#endif
```

- [ ] **Step 3: Wire `session_store.cpp` to the new helper.**

In `session_store.cpp`, add:

```cpp
#include "session_pump.h"
```

Remove includes that are no longer needed after the move. Keep `make_chunk_id`, `make_exec_session_id`, `make_touch_order`, `PollResult`, `RECENT_PROTECTION_COUNT`, `WARNING_THRESHOLD`, `PruneCandidate`, `SessionSnapshot`, `launch_live_session`, `session_snapshot_locked`, `wait_for_generation_change_locked`, pruning, start/write APIs, and session map ownership in `session_store.cpp`.

- [ ] **Step 4: Add the new source to all session-store-linked targets.**

In `crates/remote-exec-daemon-cpp/mk/sources.mk`, add `$(SOURCE_PREFIX)src/session_pump.cpp` to:

```make
BASE_SRCS
HOST_SERVER_STREAMING_SRCS
HOST_SESSION_STORE_SRCS
HOST_SERVER_RUNTIME_SRCS
HOST_SERVER_ROUTES_SRCS
```

Keep the source next to `session_store.cpp` in each list for readability.

- [ ] **Step 5: Run focused C++ checks.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-session-store
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected: all pass.

- [ ] **Step 6: Check file sizes.**

Run: `wc -l crates/remote-exec-daemon-cpp/src/session_store.cpp crates/remote-exec-daemon-cpp/src/session_pump.cpp`
Expected: `session_store.cpp` is materially smaller and pump/output-drain logic lives in `session_pump.cpp`.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/session_pump.h crates/remote-exec-daemon-cpp/src/session_pump.cpp crates/remote-exec-daemon-cpp/src/session_store.cpp crates/remote-exec-daemon-cpp/mk/sources.mk
git commit -m "refactor: split cpp session pump helpers"
```

### Task 17: Phase C1 Integration Verification

**Files:**
- No planned source edits. Verification fixes go in a separate follow-up commit.
- Test/Verify:
  - `cargo fmt --all --check`
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** full quality gate
Reason: C1 touches shared proto exports, broker routing, host exec timing, Rust daemon auth, and C++ session-store build inventories.

- [ ] **Step 1: Run Rust formatting check.**

Run: `cargo fmt --all --check`
Expected: no formatting diff.

- [ ] **Step 2: Run full workspace tests.**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 3: Run clippy with warnings denied.**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Run POSIX C++ check.**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: all host tests and POSIX daemon build pass.

- [ ] **Step 5: Run XP C++ check.**

Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
Expected: XP daemon and configured XP test build targets pass.

- [ ] **Step 6: Commit verification fixes only if needed.**

If a gate command required a code fix, commit the concrete changed files separately. For example, a formatting or import cleanup after Task 15 should use:

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/rpc/*.rs
git commit -m "fix: resolve phase c1 integration fallout"
```

If no files changed, do not create an empty commit.

## Self-Review

- Spec coverage: every C1 cleanup/refactor item from the split is represented by at least one task. C2 reliability items are explicitly deferred.
- Placeholder scan: the plan avoids open-ended placeholders and includes exact files, commands, and code shapes for each task.
- Type consistency: introduced names are consistent across tasks: `HttpAuthConfig`, `DEFAULT_TUNNEL_QUEUE_BYTES`, `TransferEndpointTarget`, `RemoteTargetHandle`, `BrokerTool`, and `session_pump`.
