# Host Runtime Port Forward Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Remove the remaining port-forward HTTP handler leakage from `remote-exec-host` while preserving the current public port-forward behavior and RPC codes.

**Architecture:** Keep `remote-exec-host` responsible for host-local port-forward state, validation, socket operations, and `HostRpcError` construction. Move all Axum request extraction and HTTP response shaping for port-forward routes into `remote-exec-daemon`, and have broker-local callers map `HostRpcError` directly instead of translating Axum tuples.

**Tech Stack:** Rust 2024, Tokio, axum, serde, existing daemon integration tests, existing broker MCP tests, cargo test, cargo fmt, cargo clippy

---

### Task 1: Make host port-forward operations transport-neutral

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-host`

**Testing approach:** `existing tests + targeted verification`
Reason: this is an internal boundary refactor. The public surface is already exercised through daemon and broker suites, so the host crate only needs to keep compiling and passing its existing coverage.

- [ ] **Step 1: Remove Axum transport imports and handler wrappers from the host module**

```rust
// Delete these imports from crates/remote-exec-host/src/port_forward.rs:
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::RpcErrorBody;

// Delete these HTTP-only wrappers entirely:
pub async fn listen(...)
pub async fn listen_accept(...)
pub async fn listen_close(...)
pub async fn connect(...)
pub async fn connection_read(...)
pub async fn connection_write(...)
pub async fn connection_close(...)
pub async fn udp_datagram_read(...)
pub async fn udp_datagram_write(...)
```

- [ ] **Step 2: Change all local entrypoints and helpers to return `HostRpcError`**

```rust
// crates/remote-exec-host/src/port_forward.rs
use crate::{AppState, HostRpcError};

pub async fn listen_local(
    state: Arc<AppState>,
    req: PortListenRequest,
) -> Result<PortListenResponse, HostRpcError> { /* existing body */ }

async fn tcp_connection(
    state: &AppState,
    connection_id: &str,
) -> Result<Arc<TcpConnection>, HostRpcError> { /* existing body */ }

fn decode_bytes(data: &str) -> Result<Vec<u8>, HostRpcError> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|err| rpc_error("invalid_port_data", err.to_string()))
}

fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    HostRpcError {
        status: 400,
        code,
        message,
    }
}
```

- [ ] **Step 3: Run the host crate verification**

Run: `cargo test -p remote-exec-host`
Expected: PASS.

### Task 2: Move port-forward HTTP wrappers into the daemon and update broker-local mapping

**Files:**
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test port_forward_rpc`, `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** `existing tests + targeted verification`
Reason: the daemon and broker already expose the public routes that matter here, so these route-level suites are the right proof that the refactor preserved behavior. Add small regression assertions only where they improve direct coverage of the refactored error-mapping seam.

- [ ] **Step 1: Replace the daemon re-export shim with Axum wrappers around host-local functions**

```rust
// crates/remote-exec-daemon/src/port_forward.rs
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectRequest, PortConnectResponse, PortConnectionCloseRequest,
    PortConnectionReadRequest, PortConnectionReadResponse, PortConnectionWriteRequest,
    PortListenAcceptRequest, PortListenAcceptResponse, PortListenCloseRequest,
    PortListenRequest, PortListenResponse, PortUdpDatagramReadRequest,
    PortUdpDatagramReadResponse, PortUdpDatagramWriteRequest, RpcErrorBody,
};

pub use remote_exec_host::port_forward::PortForwardState;

pub async fn listen(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<PortListenRequest>,
) -> Result<Json<PortListenResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::port_forward::listen_local(state, req)
        .await
        .map(Json)
        .map_err(crate::rpc_error::host_rpc_error_response)
}
```

- [ ] **Step 2: Change broker-local port-forward callers to map `HostRpcError` directly**

```rust
// crates/remote-exec-broker/src/local_backend.rs
remote_exec_host::port_forward::listen_local(self.state.clone(), req.clone())
    .await
    .map_err(map_host_rpc_error)

// crates/remote-exec-broker/src/port_forward.rs
use crate::local_backend::map_host_rpc_error;

remote_exec_host::port_forward::listen_local(self.state.clone(), req.clone())
    .await
    .map_err(map_host_rpc_error)
```

- [ ] **Step 3: Add or tighten route coverage only where it proves the new seam**

```rust
// crates/remote-exec-daemon/tests/port_forward_rpc.rs
// Add one explicit bad-request assertion such as invalid base64 on
// /v1/port/connection/write -> 400 / invalid_port_data if coverage is currently missing.

// crates/remote-exec-broker/tests/mcp_forward_ports.rs
// Keep the local open/list/close flow green; add a narrow local error assertion only if needed
// to prove HostRpcError mapping through the MCP-facing broker path.
```

- [ ] **Step 4: Run the focused daemon and broker verification**

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS with stable route behavior and any added error assertion green.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS with local forwarding behavior unchanged.

### Task 3: Run the required full gate and commit the slice

**Files:**
- Modify: none if the gate passes
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: this slice changes the shared host crate plus both daemon and broker adapters, so the workspace gate is required before claiming completion or creating the commit.

- [ ] **Step 1: Run the full workspace test gate**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 2: Run formatting and lint gates**

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 3: Commit the verified slice**

```bash
git add docs/superpowers/plans/2026-05-04-host-runtime-port-forward-boundary.md \
  crates/remote-exec-host/src/port_forward.rs \
  crates/remote-exec-daemon/src/port_forward.rs \
  crates/remote-exec-broker/src/local_backend.rs \
  crates/remote-exec-broker/src/port_forward.rs \
  crates/remote-exec-daemon/tests/port_forward_rpc.rs \
  crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "refactor: decouple host port forward from http transport"
```
