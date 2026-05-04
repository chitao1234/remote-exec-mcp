# Host Runtime Exec Patch Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Remove the remaining exec and patch HTTP handler leakage from `remote-exec-host` while preserving public RPC codes and broker-local behavior.

**Architecture:** Keep `remote-exec-host` responsible for host-local exec and patch behavior plus host-native error construction through `HostRpcError`. Move the Axum request extraction and HTTP response shaping for exec and patch into `remote-exec-daemon`, and let the broker local backend adapt `HostRpcError` directly instead of decoding Axum-shaped tuples.

**Tech Stack:** Rust 2024, Tokio, axum, serde, existing daemon integration tests, existing broker MCP tests, cargo test, cargo fmt, cargo clippy

---

### Task 1: Make host exec transport-neutral at the error and handler boundary

**Files:**
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/exec/mod.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Test/Verify: `cargo test -p remote-exec-host`, `cargo test -p remote-exec-daemon --test exec_rpc`, `cargo test -p remote-exec-broker --test mcp_exec`

**Testing approach:** `existing tests + targeted verification`
Reason: the observable exec behavior is already covered by daemon and broker suites, and this task is a boundary refactor rather than a public semantic change.

- [ ] **Step 1: Change host exec helpers to construct `HostRpcError` instead of Axum tuples**

```rust
// crates/remote-exec-host/src/exec/support.rs
use crate::{AppState, HostRpcError, config::YieldTimeOperation, host_path};

pub fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    HostRpcError {
        status: 400,
        code,
        message,
    }
}

pub fn internal_error(err: anyhow::Error) -> HostRpcError {
    let message = err.to_string();
    tracing::error!(error = %message, "daemon internal error");
    HostRpcError {
        status: 500,
        code: "internal_error",
        message,
    }
}
```

- [ ] **Step 2: Remove Axum handler wrappers from the host exec module and keep only local functions**

```rust
// crates/remote-exec-host/src/exec/handlers.rs
pub async fn exec_start_local(
    state: Arc<AppState>,
    req: ExecStartRequest,
) -> Result<ExecResponse, HostRpcError> { /* existing body, mapped to HostRpcError */ }

pub async fn exec_write_local(
    state: Arc<AppState>,
    req: ExecWriteRequest,
) -> Result<ExecResponse, HostRpcError> { /* existing body, mapped to HostRpcError */ }
```

```rust
// crates/remote-exec-host/src/exec/mod.rs
pub use handlers::{exec_start_local, exec_write_local};
pub use support::{
    ensure_sandbox_access, internal_error, resolve_input_path,
    resolve_input_path_with_windows_posix_root, resolve_workdir, rpc_error,
};
```

- [ ] **Step 3: Add daemon-local HTTP wrappers that map `HostRpcError` back to RPC responses**

```rust
// crates/remote-exec-daemon/src/exec/mod.rs
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest, RpcErrorBody};

pub use remote_exec_host::exec::{
    ensure_sandbox_access, resolve_input_path, resolve_input_path_with_windows_posix_root,
    resolve_workdir, session, store, transcript,
};

pub async fn exec_start(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::exec::exec_start_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub async fn exec_write(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::exec::exec_write_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}

pub(crate) fn rpc_error(
    code: &'static str,
    message: impl Into<String>,
) -> (StatusCode, Json<RpcErrorBody>) {
    crate::rpc_error::host_rpc_error_response(remote_exec_host::exec::rpc_error(code, message))
}

pub(crate) fn internal_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    crate::rpc_error::host_rpc_error_response(remote_exec_host::exec::internal_error(err))
}
```

- [ ] **Step 4: Switch broker-local exec calls to map `HostRpcError` directly**

```rust
// crates/remote-exec-broker/src/local_backend.rs
remote_exec_host::exec::exec_start_local(self.state.clone(), req.clone())
    .await
    .map_err(map_host_rpc_error)

remote_exec_host::exec::exec_write_local(self.state.clone(), req.clone())
    .await
    .map_err(map_host_rpc_error)
```

- [ ] **Step 5: Run the focused exec verification**

Run: `cargo test -p remote-exec-host`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test exec_rpc`
Expected: PASS with stable `stdin_closed`, `sandbox_denied`, and session error behavior.

Run: `cargo test -p remote-exec-broker --test mcp_exec`
Expected: PASS with broker-local exec and session handling unchanged.

- [ ] **Step 6: Commit the exec slice**

```bash
git add crates/remote-exec-host/src/exec/handlers.rs \
  crates/remote-exec-host/src/exec/support.rs \
  crates/remote-exec-host/src/exec/mod.rs \
  crates/remote-exec-daemon/src/exec/mod.rs \
  crates/remote-exec-broker/src/local_backend.rs \
  docs/superpowers/plans/2026-05-04-host-runtime-exec-patch-boundary.md
git commit -m "refactor: decouple host exec from http transport"
```

### Task 2: Make host patch transport-neutral at the error and handler boundary

**Files:**
- Modify: `crates/remote-exec-host/src/patch/mod.rs`
- Modify: `crates/remote-exec-daemon/src/patch/mod.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test patch_rpc`, `cargo test -p remote-exec-broker --test mcp_assets`

**Testing approach:** `existing tests + targeted verification`
Reason: patch error envelopes and successful apply output already have broad route-level coverage, so the right proof here is green focused suites after the structural change.

- [ ] **Step 1: Remove the host Axum patch handler and return `HostRpcError` from the local entrypoint**

```rust
// crates/remote-exec-host/src/patch/mod.rs
use crate::{AppState, HostRpcError};

pub async fn apply_patch_local(
    state: Arc<AppState>,
    req: PatchApplyRequest,
) -> Result<PatchApplyResponse, HostRpcError> { /* existing body */ }

fn map_patch_error(err: anyhow::Error) -> HostRpcError {
    let code = if err.downcast_ref::<SandboxError>().is_some() {
        "sandbox_denied"
    } else {
        "patch_failed"
    };
    crate::exec::rpc_error(code, err.to_string())
}
```

- [ ] **Step 2: Add the daemon-local patch HTTP wrapper**

```rust
// crates/remote-exec-daemon/src/patch/mod.rs
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse, RpcErrorBody};

pub async fn apply_patch(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::patch::apply_patch_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}
```

- [ ] **Step 3: Switch broker-local patch calls to map `HostRpcError` directly**

```rust
// crates/remote-exec-broker/src/local_backend.rs
remote_exec_host::patch::apply_patch_local(self.state.clone(), req.clone())
    .await
    .map_err(map_host_rpc_error)
```

- [ ] **Step 4: Run the focused patch verification**

Run: `cargo test -p remote-exec-daemon --test patch_rpc`
Expected: PASS with stable `patch_failed` and `sandbox_denied` responses.

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: PASS with broker-local `apply_patch` behavior unchanged.

- [ ] **Step 5: Commit the patch slice**

```bash
git add crates/remote-exec-host/src/patch/mod.rs \
  crates/remote-exec-daemon/src/patch/mod.rs \
  crates/remote-exec-broker/src/local_backend.rs \
  docs/superpowers/plans/2026-05-04-host-runtime-exec-patch-boundary.md
git commit -m "refactor: decouple host patch from http transport"
```

### Task 3: Run the final gate for the shared host-runtime change

**Files:**
- Modify: none if the gate passes
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: this slice changes the shared host crate plus both daemon and broker adapters, so the repo-wide gate is required before claiming completion or committing the final state.

- [ ] **Step 1: Run the workspace test gate**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 2: Run formatting and lint gates**

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 3: If any gate fails, fix only the reported issue and re-run that gate**

```text
Do not infer success from a previous run. Re-run the exact failing command until it is clean.
```
