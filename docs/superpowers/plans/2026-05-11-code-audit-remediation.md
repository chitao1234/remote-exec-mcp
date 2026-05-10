# Code Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve the still-valid and partially-addressed findings from `docs/CODE_AUDIT.md` without reviving stale historical findings or changing documented public behavior accidentally.

**Architecture:** Treat `docs/CODE_AUDIT.md` as review input, not as the live contract. Start with protocol/data-shape cleanup that reduces broad stringly and duplicate plumbing, then handle Rust port-forward structure, daemon lifecycle/error cleanup, C++ reliability/build cleanup, and PKI/admin security polish. Each task is independently commit-worthy and keeps behavior stable unless the task explicitly tightens an invariant.

**Tech Stack:** Rust 2024 workspace with Tokio, Axum/Hyper, Serde/Schemars, existing broker/daemon/host tests; C++11-compatible daemon with GNU make and BSD make paths; rcgen-based PKI/admin crates.

---

## Findings Covered

Still valid: `#1`, `#2`, `#3`, `#4`, `#5`, `#6`, `#7`, `#8`, `#9`, `#10`, `#12`, `#13`, `#14`, `#16`, `#17`, `#18`, `#19`, `#22`, `#24`, `#26`, `#27`, `#28`, `#30`, `#31`, `#33`, `#35`.

Partially addressed and included for final cleanup or confirmation: `#11`, `#20`, `#21`, `#23`, `#33`.

Already fixed and not planned except for regression checks: `#15`, `#25`, `#29`, `#32`, `#34`, `#36`.

---

### Task 1: Save This Remediation Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-code-audit-remediation.md`
- Test/Verify: `git status --short`

**Testing approach:** no new tests needed
Reason: This task only adds the implementation plan artifact.

- [ ] **Step 1: Add this plan file.**

This file is the plan.

- [ ] **Step 2: Verify the plan is the only intended change.**

Run: `git status --short`
Expected: only `docs/superpowers/plans/2026-05-11-code-audit-remediation.md` is new or staged.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-code-audit-remediation.md
git commit -m "docs: plan code audit remediation"
```

### Task 2: Consolidate Transfer Protocol Types

**Findings:** `#1`

**Files:**
- Create: `crates/remote-exec-proto/src/transfer.rs`
- Modify: `crates/remote-exec-proto/src/lib.rs`
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/format.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/codec.rs`
- Modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/codec.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-host/src/transfer/**/*.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto`
  - `cargo test -p remote-exec-broker --test mcp_transfer`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`

**Testing approach:** existing tests + compile-driven refactor
Reason: The wire values must remain stable. Existing transfer tests cover public JSON, local transfer, broker-to-daemon transfer, and archive headers.

- [ ] **Step 1: Create canonical transfer types in `remote-exec-proto`.**

Create `crates/remote-exec-proto/src/transfer.rs` with the canonical public/RPC shared types:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
    Multiple,
}

impl TransferSourceType {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Multiple => "multiple",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "file" => Some(Self::File),
            "directory" => Some(Self::Directory),
            "multiple" => Some(Self::Multiple),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwrite {
    Fail,
    #[default]
    Merge,
    Replace,
}

impl TransferOverwrite {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Merge => "merge",
            Self::Replace => "replace",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "fail" => Some(Self::Fail),
            "merge" => Some(Self::Merge),
            "replace" => Some(Self::Replace),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSymlinkMode {
    #[default]
    Preserve,
    Follow,
    Skip,
}

impl TransferSymlinkMode {
    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Follow => "follow",
            Self::Skip => "skip",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "preserve" => Some(Self::Preserve),
            "follow" => Some(Self::Follow),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferExportRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "crate::rpc::TransferCompression::is_none")]
    pub compression: crate::rpc::TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExportMetadata {
    pub source_type: TransferSourceType,
    pub compression: crate::rpc::TransferCompression,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportRequest {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: crate::rpc::TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferImportMetadata {
    pub destination_path: String,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: crate::rpc::TransferCompression,
    pub symlink_mode: TransferSymlinkMode,
}

impl TransferImportRequest {
    pub fn metadata(&self) -> TransferImportMetadata {
        TransferImportMetadata {
            destination_path: self.destination_path.clone(),
            overwrite: self.overwrite.clone(),
            create_parent: self.create_parent,
            source_type: self.source_type.clone(),
            compression: self.compression.clone(),
            symlink_mode: self.symlink_mode.clone(),
        }
    }
}

impl From<TransferImportMetadata> for TransferImportRequest {
    fn from(value: TransferImportMetadata) -> Self {
        Self {
            destination_path: value.destination_path,
            overwrite: value.overwrite,
            create_parent: value.create_parent,
            source_type: value.source_type,
            compression: value.compression,
            symlink_mode: value.symlink_mode,
        }
    }
}
```

Add `pub mod transfer;` to `crates/remote-exec-proto/src/lib.rs`.

- [ ] **Step 2: Re-export canonical types from public/RPC modules.**

In `public.rs`, remove local `TransferOverwrite`, `TransferSymlinkMode`, and `TransferSourceType` definitions. Replace them with:

```rust
pub use crate::transfer::{TransferOverwrite, TransferSourceType, TransferSymlinkMode};
```

In `rpc.rs`, remove local transfer source/overwrite/symlink enums and request/metadata duplicates. Replace them with:

```rust
pub use crate::transfer::{
    TransferExportMetadata, TransferExportRequest, TransferImportMetadata, TransferImportRequest,
    TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};
```

Update references from `TransferOverwriteMode` to `TransferOverwrite`. Update existing `TransferImportMetadata::from(req)` call sites to `req.metadata()`.

- [ ] **Step 3: Remove conversion boilerplate at broker boundaries.**

In `crates/remote-exec-broker/src/tools/transfer/operations.rs`, remove `to_rpc_overwrite_mode` and `to_rpc_symlink_mode`. Build `TransferImportRequest` directly from the public enum values:

```rust
TransferImportRequest {
    destination_path,
    overwrite: overwrite.clone(),
    create_parent,
    source_type,
    compression,
    symlink_mode: symlink_mode.clone(),
}
```

In `format.rs`, remove conversion from RPC source type to public source type; `TransferFilesResult.source_type` can receive the shared enum directly.

- [ ] **Step 4: Update header parsing and test imports.**

Change all imports of `TransferOverwriteMode` to `TransferOverwrite`. Header parsing should still accept and emit the same strings through `wire_value()` and `from_wire_value()`.

- [ ] **Step 5: Run focused verification.**

Run:

```bash
cargo test -p remote-exec-proto
cargo test -p remote-exec-broker --test mcp_transfer
cargo test -p remote-exec-daemon --test transfer_rpc
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-proto/src/transfer.rs crates/remote-exec-proto/src/lib.rs crates/remote-exec-proto/src/public.rs crates/remote-exec-proto/src/rpc.rs crates/remote-exec-broker/src crates/remote-exec-daemon/src crates/remote-exec-host/src
git commit -m "refactor: share transfer protocol types"
```

### Task 3: Promote RPC Error And Warning Codes To Typed Proto Values

**Findings:** `#2`, residual `#21`

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-proto/src/port_tunnel.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-daemon/src/http/auth.rs`
- Modify: `crates/remote-exec-daemon/src/http/version.rs`
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Modify: `crates/remote-exec-daemon/src/rpc_error.rs`
- Modify: `crates/remote-exec-host/src/error.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/port_forward/error.rs`
- Modify: `crates/remote-exec-host/src/port_forward/limiter.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session_store.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto`
  - `cargo test -p remote-exec-host port_forward`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`
  - `cargo test -p remote-exec-daemon --test health`

**Testing approach:** characterization + existing tests
Reason: The HTTP/JSON wire body should still serialize codes as stable snake-case strings, while Rust code stops comparing raw string literals.

- [ ] **Step 1: Add canonical code enums in `rpc.rs`.**

Add:

```rust
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
    PatchFailed,
    Internal,
}

impl RpcErrorCode {
    pub fn wire_value(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::UnknownSession => "unknown_session",
            Self::NotFound => "not_found",
            Self::UnknownEndpoint => "unknown_endpoint",
            Self::InvalidPortTunnel => "invalid_port_tunnel",
            Self::PortTunnelUnavailable => "port_tunnel_unavailable",
            Self::PortTunnelLimitExceeded => "port_tunnel_limit_exceeded",
            Self::PatchFailed => "patch_failed",
            Self::Internal => "internal",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "bad_request" => Some(Self::BadRequest),
            "unauthorized" => Some(Self::Unauthorized),
            "unknown_session" => Some(Self::UnknownSession),
            "not_found" => Some(Self::NotFound),
            "unknown_endpoint" => Some(Self::UnknownEndpoint),
            "invalid_port_tunnel" => Some(Self::InvalidPortTunnel),
            "port_tunnel_unavailable" => Some(Self::PortTunnelUnavailable),
            "port_tunnel_limit_exceeded" => Some(Self::PortTunnelLimitExceeded),
            "patch_failed" => Some(Self::PatchFailed),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    SessionLimitApproaching,
    TransferSkippedUnsupportedEntry,
    TransferSkippedSymlink,
}

impl WarningCode {
    pub fn wire_value(self) -> &'static str {
        match self {
            Self::SessionLimitApproaching => "session_limit_approaching",
            Self::TransferSkippedUnsupportedEntry => "transfer_skipped_unsupported_entry",
            Self::TransferSkippedSymlink => "transfer_skipped_symlink",
        }
    }
}
```

- [ ] **Step 2: Keep wire strings but type constructors.**

Keep `RpcErrorBody.code`, `ExecWarning.code`, and `TransferWarning.code` as `String` for compatibility. Add constructors:

```rust
impl RpcErrorBody {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.wire_value().to_string(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> Option<RpcErrorCode> {
        RpcErrorCode::from_wire_value(&self.code)
    }
}
```

Update `ExecWarning` and `TransferWarning` constructors to use `WarningCode`.

- [ ] **Step 3: Replace broker-local `RpcErrorCode`.**

Remove the duplicate enum from `crates/remote-exec-broker/src/daemon_client.rs` and import `remote_exec_proto::rpc::RpcErrorCode`. Keep these methods on `DaemonClientError`:

```rust
pub fn rpc_error_code(&self) -> Option<RpcErrorCode> {
    self.rpc_code().and_then(RpcErrorCode::from_wire_value)
}

pub fn is_rpc_error_code(&self, expected: RpcErrorCode) -> bool {
    self.rpc_error_code() == Some(expected)
}
```

- [ ] **Step 4: Replace raw string creation and comparison.**

Use `RpcErrorBody::new(...)` and `RpcErrorCode` in:
- daemon `bad_request`, auth unauthorized, port-forward bad upgrade request
- host `rpc_error` helpers
- port-tunnel limit metadata checks and tests

Leave JSON metadata fields as strings where the protocol frame metadata is JSON, but source those strings from `RpcErrorCode::PortTunnelLimitExceeded.wire_value()`.

- [ ] **Step 5: Run focused verification.**

Run:

```bash
cargo test -p remote-exec-proto
cargo test -p remote-exec-host port_forward
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-broker --test mcp_forward_ports
cargo test -p remote-exec-daemon --test health
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-proto/src/port_tunnel.rs crates/remote-exec-broker/src crates/remote-exec-daemon/src crates/remote-exec-host/src
git commit -m "refactor: type rpc error codes"
```

### Task 4: Centralize Opaque ID Generation

**Findings:** `#4`, `#26`

**Files:**
- Create: `crates/remote-exec-host/src/ids.rs`
- Modify: `crates/remote-exec-host/src/lib.rs`
- Modify: `crates/remote-exec-host/src/state.rs`
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-broker/src/session_store.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Test/Verify:
  - `cargo test -p remote-exec-host`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`
  - `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** existing tests + small unit tests
Reason: IDs are opaque, but prefixes leak implementation identity and scattered format changes can break assumptions in tests.

- [ ] **Step 1: Add Rust ID helpers.**

Create `crates/remote-exec-host/src/ids.rs`:

```rust
fn uuid_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn new_instance_id() -> String {
    format!("inst_{}", uuid_suffix())
}

pub fn new_exec_session_id() -> String {
    format!("sess_{}", uuid_suffix())
}

pub fn new_tunnel_session_id() -> String {
    format!("ptun_{}", uuid_suffix())
}

pub fn new_public_session_id() -> String {
    format!("sess_{}", uuid_suffix())
}

pub fn new_forward_id() -> String {
    format!("fwd_{}", uuid_suffix())
}
```

Export it from `lib.rs` with `pub mod ids;`.

- [ ] **Step 2: Replace Rust ad-hoc ID generation.**

Use:
- `ids::new_instance_id()` in host state
- `ids::new_exec_session_id()` in host exec handlers
- `ids::new_tunnel_session_id()` in host port-forward tunnel
- `ids::new_public_session_id()` in broker session store
- `ids::new_forward_id()` in broker port-forward supervisor

- [ ] **Step 3: Add C++ opaque ID helper.**

Create a small helper in the relevant C++ translation units or a shared utility if already convenient:

```cpp
std::string next_opaque_id(const char* prefix, unsigned long sequence) {
    std::ostringstream out;
    out << prefix << platform::monotonic_ms() << "_" << sequence;
    return out.str();
}
```

Use `"sess_"` for exec session IDs and `"ptun_"` for tunnel session IDs. Do not include `"cpp"` in either ID.

- [ ] **Step 4: Update tests that asserted implementation-specific prefixes.**

Search:

```bash
rg -n '"cpp-|sess_cpp_|sess_|ptun_|fwd_|inst_' crates tests
```

Keep assertions about opaque ID presence and target/session isolation. Avoid assertions that require daemon implementation markers.

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-host
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-broker --test mcp_forward_ports
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src/ids.rs crates/remote-exec-host/src/lib.rs crates/remote-exec-host/src/state.rs crates/remote-exec-host/src/exec/handlers.rs crates/remote-exec-host/src/port_forward/tunnel.rs crates/remote-exec-broker/src/session_store.rs crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-daemon-cpp/src/session_store.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp
git commit -m "refactor: centralize opaque ids"
```

### Task 5: Tighten Always-Present And Timestamp Fields

**Findings:** `#5`

**Files:**
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec_format.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Modify: tests that construct `ListTargetDaemonInfo`, `ExecResponse`, or `ForwardPortEntry`
- Test/Verify:
  - `cargo test -p remote-exec-proto`
  - `cargo test -p remote-exec-host exec`
  - `cargo test -p remote-exec-broker --test mcp_exec`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** compile-driven refactor + existing tests
Reason: Tightening fields is API-shape work. Tests should catch schema and formatting regressions.

- [ ] **Step 1: Make `port_forward_protocol_version` explicit in daemon info output.**

Change:

```rust
pub port_forward_protocol_version: Option<u32>,
```

to:

```rust
pub port_forward_protocol_version: u32,
```

In `tools/targets.rs`, emit `0` when the target does not support port forwarding instead of omitting the field.

- [ ] **Step 2: Introduce a timestamp newtype for port-forward entries.**

In `public.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Timestamp(pub String);
```

Then change:

```rust
pub last_reconnect_at: Option<String>,
```

to:

```rust
pub last_reconnect_at: Option<Timestamp>,
```

Update `unix_timestamp_string()` callers to wrap `Timestamp(unix_timestamp_string())`. This keeps the public JSON string shape if `Timestamp` uses `#[serde(transparent)]`:

```rust
#[serde(transparent)]
pub struct Timestamp(pub String);
```

- [ ] **Step 3: Split exec start/write response internally while preserving public formatting.**

Add new RPC structs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecStartResponse {
    pub daemon_session_id: String,
    pub response: ExecResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWriteResponse {
    pub response: ExecResponse,
}
```

Keep daemon HTTP and C++ daemon wire bodies as `ExecResponse` in this task. Use the split structs only inside Rust host/broker code to make invariants local and truthful without changing the C++ daemon or documented daemon HTTP JSON shape.

In `remote-exec-host`, have `exec_start_local` construct `ExecStartResponse` internally, then convert to the existing `ExecResponse` for the daemon endpoint. Have broker formatting consume `ExecResponse` defensively but move session-id-required logic to the start path only. Do not require `daemon_session_id` for write/poll responses.

- [ ] **Step 4: Update tests and schemas.**

Search and fix constructors:

```bash
rg -n "port_forward_protocol_version:|last_reconnect_at:|daemon_session_id:" crates tests
```

Tests should assert that unsupported port-forward targets report version `0`.

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-proto
cargo test -p remote-exec-host exec
cargo test -p remote-exec-broker --test mcp_exec
cargo test -p remote-exec-broker --test mcp_forward_ports
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-proto/src/public.rs crates/remote-exec-proto/src/rpc.rs crates/remote-exec-broker/src crates/remote-exec-host/src tests
git commit -m "refactor: tighten protocol field invariants"
```

### Task 6: Share Daemon TLS And Plain HTTP Serve Loops

**Findings:** `#3`

**Files:**
- Create: `crates/remote-exec-daemon/src/http_serve.rs`
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/tls.rs`
- Modify: `crates/remote-exec-daemon/src/tls_enabled.rs`
- Test/Verify:
  - `cargo test -p remote-exec-daemon --test health`
  - `cargo test -p remote-exec-daemon --test exec_rpc`
  - `cargo test -p remote-exec-daemon --features tls --test health`

**Testing approach:** existing integration tests
Reason: The behavior should not change; the seam is listener accept plus optional TLS wrapping before a shared HTTP/1 connection loop.

- [ ] **Step 1: Extract shared connection serving.**

Create `http_serve.rs` with:

```rust
use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use futures_util::future::BoxFuture;
use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tower::ServiceExt;

pub trait AcceptedStreamIo: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}

impl<T> AcceptedStreamIo for T where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}

pub type AcceptedStream = Box<dyn AcceptedStreamIo>;
pub type AcceptStream = Arc<
    dyn Fn(TcpStream) -> BoxFuture<'static, anyhow::Result<Option<AcceptedStream>>> + Send + Sync,
>;

pub async fn serve_http1_connections<F>(
    listener: TcpListener,
    app: Router,
    shutdown: F,
    accept_stream: AcceptStream,
    log_label: &'static str,
) -> anyhow::Result<()>
where
    F: Future<Output = ()> + Send,
{
    let mut connections = JoinSet::new();
    let (connection_shutdown_tx, _) = watch::channel(());
    tokio::pin!(shutdown);

    loop {
        while let Some(result) = connections.try_join_next() {
            if let Err(err) = result {
                tracing::warn!(?err, "connection task failed");
            }
        }

        tokio::select! {
            _ = &mut shutdown => {
                break;
            }
            accepted = listener.accept() => {
                let (stream, peer_addr) = accepted?;
                tracing::debug!(peer = %peer_addr, transport = log_label, "accepted tcp connection");
                let app = app.clone();
                let accept_stream = accept_stream.clone();
                let mut connection_shutdown = connection_shutdown_tx.subscribe();
                connections.spawn(async move {
                    let stream = match accept_stream(stream).await {
                        Ok(Some(stream)) => stream,
                        Ok(None) => return,
                        Err(err) => {
                            tracing::warn!(peer = %peer_addr, ?err, transport = log_label, "connection accept failed");
                            return;
                        }
                    };

                    let io = TokioIo::new(stream);
                    let service = service_fn(move |request: Request<Incoming>| {
                        let app = app.clone();
                        async move { app.oneshot(request.map(Body::new)).await }
                    });
                    let connection = http1::Builder::new()
                        .serve_connection(io, service)
                        .with_upgrades();
                    tokio::pin!(connection);

                    tokio::select! {
                        result = &mut connection => {
                            if let Err(err) = result {
                                tracing::warn!(peer = %peer_addr, ?err, "http serve failed");
                            }
                        }
                        changed = connection_shutdown.changed() => {
                            if changed.is_ok() {
                                connection.as_mut().graceful_shutdown();
                            }
                            if let Err(err) = connection.await {
                                tracing::warn!(peer = %peer_addr, ?err, "http serve failed during shutdown");
                            }
                        }
                    }
                });
            }
        }
    }

    drop(listener);
    let _ = connection_shutdown_tx.send(());

    while let Some(result) = connections.join_next().await {
        if let Err(err) = result {
            tracing::warn!(?err, "connection task failed during shutdown");
        }
    }

    tracing::info!(transport = log_label, "daemon listener stopped");
    Ok(())
}
```

- [ ] **Step 2: Use the helper for plain HTTP.**

In `tls.rs`, `serve_http_with_shutdown` should bind the listener, log the HTTP listener, and call `serve_http1_connections` with an acceptor that boxes the raw `TcpStream`.

- [ ] **Step 3: Use the helper for TLS.**

In `tls_enabled.rs`, `serve_tls_with_shutdown` should bind the listener, build `TlsAcceptor`, and call `serve_http1_connections` with an acceptor that performs `tls.accept(stream).await`. TLS accept failures should still log `tls accept failed` and continue.

- [ ] **Step 4: Run verification.**

Run:

```bash
cargo test -p remote-exec-daemon --test health
cargo test -p remote-exec-daemon --test exec_rpc
cargo test -p remote-exec-daemon --features tls --test health
```

Expected: all pass. If TLS feature test is unsupported in the current environment, record the exact compile/runtime error before committing.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon/src/http_serve.rs crates/remote-exec-daemon/src/lib.rs crates/remote-exec-daemon/src/tls.rs crates/remote-exec-daemon/src/tls_enabled.rs
git commit -m "refactor: share daemon http serve loop"
```

### Task 7: Finish Rust Production Panic And Re-Export Audit

**Findings:** partially addressed `#11`, `#20`

**Files:**
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs` if the scan finds unwraps outside `#[cfg(test)]`
- Modify: `crates/remote-exec-pki/src/write.rs` if the scan finds production `expect`/`unwrap`
- Test/Verify:
  - `cargo test --workspace --lib`
  - targeted tests for touched crates

**Testing approach:** fresh audit + targeted verification
Reason: The original line references are stale. This task prevents obsolete findings from turning into churn.

- [ ] **Step 1: Run a production-only panic scan.**

Run:

```bash
rg -n "expect\\(|unwrap\\(" crates --glob '!**/tests.rs' --glob '!**/tests/**'
rg -n "pub use remote_exec_host::exec|pub use remote_exec_host::transfer|pub use remote_exec_host::patch" crates/remote-exec-daemon/src
```

Expected: test-only unwraps are ignored. Any remaining production unwrap/expect must be manually classified as safe invariant, error-propagation bug, or FFI/build limitation.

- [ ] **Step 2: Downgrade unnecessary daemon public re-export.**

If `remote_exec_daemon::exec::session` has no external use, change:

```rust
pub use remote_exec_host::exec::session;
```

to:

```rust
pub(crate) use remote_exec_host::exec::session;
```

If tests outside the crate require it, leave the re-export and document the reason in the commit message.

- [ ] **Step 3: Fix any true production unwraps by propagation.**

Use `anyhow::Context` at binary/config/client boundaries and `unwrap_or` only when there is a deterministic fallback. Do not change test helpers.

- [ ] **Step 4: Run verification.**

Run:

```bash
cargo test --workspace --lib
```

Expected: all library tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon/src crates/remote-exec-broker/src crates/remote-exec-pki/src
git commit -m "refactor: finish production panic audit"
```

If no files changed, do not create an empty commit. Mark the task complete in execution notes as "no true positives found".

### Task 8: Reshape Broker Listen Session State And Forward Task Ownership

**Findings:** `#8`, `#9`

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: tests in those modules
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** existing port-forward tests
Reason: This is state ownership cleanup with no protocol behavior change. Current tests cover open, close, reconnect, and close-after-task-stop behavior.

- [ ] **Step 1: Replace split listen-session locks with one state lock.**

Use:

```rust
struct ListenSessionState {
    current_tunnel: Option<Arc<PortTunnel>>,
}

pub(super) struct ListenSessionControl {
    listen_side: SideHandle,
    listen_stream_id: u32,
    state: Mutex<ListenSessionState>,
}
```

`with_exclusive_operation` should lock `state` once and pass `&mut ListenSessionState` to the closure or perform operations in methods that hold this single guard.

- [ ] **Step 2: Keep tunnel replacement and reads explicit.**

Provide methods:

```rust
async fn current_tunnel(&self) -> Option<Arc<PortTunnel>>;
async fn replace_current_tunnel(&self, tunnel: Arc<PortTunnel>);
async fn with_session_state<F, T>(&self, op: F) -> T
where
    F: FnOnce(&mut ListenSessionState) -> T;
```

If any operation must await while holding state, split it so the guard is released before awaiting on tunnel I/O.

- [ ] **Step 3: Let the store own forward task handles.**

Replace `Arc<Mutex<Option<JoinHandle<()>>>>` with a store-owned close path:

```rust
pub(super) struct PortForwardRecord {
    entry: ForwardPortEntry,
    close_handle: PortForwardCloseHandle,
}

pub(super) struct PortForwardCloseHandle {
    control: Arc<ListenSessionControl>,
    task: Mutex<Option<JoinHandle<()>>>,
}
```

`OpenedForward` should return the task only once to the store. External close callers receive a forward ID, not shared direct access to the join handle.

- [ ] **Step 4: Update close path tests.**

Tests should assert:
- closing a forward closes listener and waits for task stop
- closing the same forward twice is harmless at the public API level
- task handle is consumed once

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_forward_ports
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-broker/src/port_forward/store.rs
git commit -m "refactor: simplify broker forward state ownership"
```

### Task 9: Extract Host Port-Forward Types And Timings

**Findings:** `#6`, `#16`

**Files:**
- Create: `crates/remote-exec-host/src/port_forward/types.rs`
- Create: `crates/remote-exec-host/src/port_forward/timings.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Test/Verify: `cargo test -p remote-exec-host port_forward`

**Testing approach:** existing unit tests
Reason: This is structural extraction; existing host port-forward tests provide the behavior safety net.

- [ ] **Step 1: Move data-only types to `types.rs`.**

Move these definitions from `mod.rs` without changing fields or visibility:
- `TunnelState`
- `TunnelSender`
- `TunnelMode`
- `TcpStreamEntry`
- `TcpWriterHandle`
- `UdpReaderEntry`
- `TransportUdpBind`
- `ErrorMeta`
- `EndpointMeta`
- `QueuedFrame`
- `TcpWriteCommand`
- `EndpointOkMeta`
- `TcpAcceptMeta`
- `UdpDatagramMeta`

In `mod.rs`, re-export:

```rust
mod types;
pub(crate) use types::{
    EndpointMeta, ErrorMeta, TcpStreamEntry, TcpWriterHandle, TransportUdpBind, TunnelMode,
    TunnelSender, TunnelState, UdpReaderEntry,
};
```

- [ ] **Step 2: Move timing constants behind `PortTunnelTimings`.**

Create:

```rust
#[derive(Debug, Clone, Copy)]
pub(crate) struct PortTunnelTimings {
    pub resume_timeout: std::time::Duration,
}

impl PortTunnelTimings {
    pub(crate) fn production() -> Self {
        Self {
            resume_timeout: std::time::Duration::from_secs(10),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            resume_timeout: std::time::Duration::from_millis(100),
        }
    }
}

pub(crate) fn timings() -> PortTunnelTimings {
    #[cfg(test)]
    {
        PortTunnelTimings::for_test()
    }
    #[cfg(not(test))]
    {
        PortTunnelTimings::production()
    }
}
```

Replace `RESUME_TIMEOUT` reads with `timings().resume_timeout`.

- [ ] **Step 3: Keep `mod.rs` as facade plus tests.**

After moving types/timings, `mod.rs` should primarily contain module declarations, public functions, and existing tests. Do not move tests unless a test becomes clearer next to the extracted helper.

- [ ] **Step 4: Run verification.**

Run:

```bash
cargo test -p remote-exec-host port_forward
```

Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-host/src/port_forward/mod.rs crates/remote-exec-host/src/port_forward/types.rs crates/remote-exec-host/src/port_forward/timings.rs crates/remote-exec-host/src/port_forward/session.rs crates/remote-exec-host/src/port_forward/tunnel.rs crates/remote-exec-host/src/port_forward/tcp.rs crates/remote-exec-host/src/port_forward/udp.rs
git commit -m "refactor: extract host port forward types"
```

### Task 10: Consolidate Broker TCP And UDP Bridge Structure

**Findings:** `#7`

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Create: `crates/remote-exec-broker/src/port_forward/bridge_events.rs`
- Create: `crates/remote-exec-broker/src/port_forward/udp_connectors.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`
  - `cargo test -p remote-exec-broker port_forward`

**Testing approach:** existing tests plus focused helper unit tests
Reason: This is a high-risk maintainability refactor. Use existing integration behavior plus direct tests for the new UDP connector map.

- [ ] **Step 1: Extract `UdpConnectorMap`.**

Create a single-lock state:

```rust
struct UdpConnectorMap {
    inner: Mutex<UdpConnectorMapState>,
}

struct UdpConnectorMapState {
    connector_by_peer: HashMap<String, UdpPeerConnector>,
    peer_by_connector: HashMap<u32, String>,
}
```

Methods:

```rust
async fn get_mut_by_peer<F, T>(&self, peer: &str, op: F) -> Option<T>
where
    F: FnOnce(&mut UdpPeerConnector) -> T;
async fn insert(&self, peer: String, stream_id: u32, connector: UdpPeerConnector);
async fn remove_by_stream_id(&self, stream_id: u32) -> Option<UdpPeerConnector>;
async fn sweep_idle(&self, now: Instant, idle_timeout: Duration) -> Vec<(u32, UdpPeerConnector)>;
async fn len(&self) -> usize;
```

Add unit tests for insert, peer lookup, stream removal, and idle sweep.

- [ ] **Step 2: Centralize drop/accounting record helpers on `ForwardRuntime`.**

Add methods:

```rust
impl ForwardRuntime {
    pub(super) async fn record_dropped_datagram(&self) -> anyhow::Result<()> { ... }
    pub(super) async fn record_dropped_stream(&self) -> anyhow::Result<()> { ... }
    pub(super) async fn mark_reconnecting(&self, side: ForwardSide, reason: &str) -> anyhow::Result<()> { ... }
    pub(super) async fn mark_active(&self, side: ForwardSide) -> anyhow::Result<()> { ... }
}
```

Replace repeated `store.update_entry` closures in both bridge files.

- [ ] **Step 3: Extract common recoverable event classification.**

Create a helper that accepts the side label and frame result and returns:

```rust
enum TunnelEvent<T> {
    Frame(T),
    Recoverable { side: ForwardSide, reason: String },
    Fatal(anyhow::Error),
}
```

Use it in both TCP and UDP loops before protocol-specific frame handling.

- [ ] **Step 4: Split `open_protocol_forward` context construction.**

In `supervisor.rs`, split the 115-line open function into:
- `open_listen_session_for_forward`
- `open_connect_tunnel_for_forward`
- `build_forward_record`

Ensure the listen/connect error context strings are built once by a helper:

```rust
fn open_context(kind: ForwardOpenKind, side: ForwardSide, target: &str) -> String
```

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-broker port_forward
cargo test -p remote-exec-broker --test mcp_forward_ports
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward
git commit -m "refactor: consolidate broker forward bridges"
```

### Task 11: Unify Host And Daemon Domain Error Mapping

**Findings:** `#10`, `#12`, `#13`

**Files:**
- Modify: `crates/remote-exec-host/src/error.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/port_forward/error.rs`
- Modify: `crates/remote-exec-host/src/transfer/archive/**/*.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-daemon/src/image.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/src/rpc_error.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host transfer`
  - `cargo test -p remote-exec-daemon --test image_rpc`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`
  - `cargo test -p remote-exec-daemon --test patch_rpc`

**Testing approach:** existing tests + compile-driven refactor
Reason: Behavior is unchanged, but error path coverage matters because HTTP status/code mapping must stay stable.

- [ ] **Step 1: Add shared host RPC conversion helpers.**

In `remote-exec-host/src/error.rs`, add:

```rust
pub(crate) fn rpc_error(
    status: u16,
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    HostRpcError {
        status,
        body: remote_exec_proto::rpc::RpcErrorBody::new(code, message),
    }
}

pub(crate) fn bad_request(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    rpc_error(400, code, message)
}

pub(crate) fn internal(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    rpc_error(500, code, message)
}
```

- [ ] **Step 2: Replace duplicated `rpc_error` helpers.**

Remove or delegate:
- `remote-exec-host/src/exec/support.rs::rpc_error`
- `remote-exec-host/src/port_forward/error.rs::rpc_error`

Use the shared helper from `host::error`.

- [ ] **Step 3: Normalize daemon handler mapping.**

Implement:

```rust
impl From<remote_exec_host::ImageError> for remote_exec_host::HostRpcError { ... }
impl From<remote_exec_host::TransferError> for remote_exec_host::HostRpcError { ... }
```

Then daemon handlers can use:

```rust
.map_err(Into::into)
.map_err(host_rpc_error_response)
```

or a small local helper:

```rust
fn domain_error_response<E>(err: E) -> (StatusCode, Json<RpcErrorBody>)
where
    E: Into<HostRpcError>,
{
    host_rpc_error_response(err.into())
}
```

- [ ] **Step 4: Convert transfer archive public functions to `TransferError`.**

Change archive functions that directly expose transfer domain behavior from `anyhow::Result<T>` to `Result<T, TransferError>`. Keep `anyhow` inside private helpers when the error is genuinely internal and convert at the boundary with `TransferError::internal(err.to_string())`.

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-host transfer
cargo test -p remote-exec-daemon --test image_rpc
cargo test -p remote-exec-daemon --test transfer_rpc
cargo test -p remote-exec-daemon --test patch_rpc
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src crates/remote-exec-daemon/src
git commit -m "refactor: unify host rpc error mapping"
```

### Task 12: Track Daemon Port-Forward Tunnel Tasks

**Findings:** `#14`

**Files:**
- Modify: `crates/remote-exec-host/src/state.rs`
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Test/Verify:
  - `cargo test -p remote-exec-daemon --test health`
  - `cargo test -p remote-exec-host port_forward`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** existing tests + lifecycle helper test
Reason: The host runtime already has a shutdown token. The daemon tunnel upgrade task should be joined or observe that token instead of being detached.

- [ ] **Step 1: Add a background task tracker to host runtime state.**

Add a small task tracker to `remote-exec-host/src/state.rs`:

```rust
#[derive(Clone, Default)]
pub struct BackgroundTasks {
    tasks: Arc<tokio::sync::Mutex<tokio::task::JoinSet<()>>>,
}

impl BackgroundTasks {
    pub async fn spawn<F>(&self, name: &'static str, task: F)
    where
        F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.tasks.lock().await.spawn(async move {
            if let Err(err) = task.await {
                tracing::warn!(task = name, ?err, "background task failed");
            }
        });
    }

    pub async fn join_all(&self) {
        let mut tasks = self.tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                tracing::warn!(?err, "background task join failed");
            }
        }
    }
}
```

Add `pub background_tasks: BackgroundTasks` to `HostRuntimeState` and initialize it in `build_runtime_state`.

- [ ] **Step 2: Replace fire-and-forget spawn with tracked or cancellable task.**

Current shape:

```rust
tokio::spawn(async move {
    remote_exec_host::port_forward::serve_tunnel(state, upgraded).await
});
```

Target shape:

```rust
let shutdown = state.shutdown.clone();
state.background_tasks.spawn("port-forward tunnel", async move {
    tokio::select! {
        result = remote_exec_host::port_forward::serve_tunnel_with_permit(
            state,
            TokioIo::new(upgraded),
            connection_permit,
        ) => result.map_err(|err| anyhow::anyhow!("{}: {}", err.code, err.message)),
        _ = shutdown.cancelled() => Ok(()),
    }
}).await;
```

In `run_until`, after `server::serve_with_shutdown(...)` returns, call `state.background_tasks.join_all().await` before returning.

- [ ] **Step 3: Add a lifecycle regression test.**

The test should:
- open a tunnel upgrade
- cancel `state.shutdown`
- assert the tunnel task exits without waiting for socket EOF

Place this as a host-level port-forward test if the daemon HTTP harness cannot observe the join directly. The test must exercise the same `state.shutdown` token used by the daemon task.

- [ ] **Step 4: Run verification.**

Run:

```bash
cargo test -p remote-exec-daemon --test health
cargo test -p remote-exec-broker --test mcp_forward_ports
```

Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon/src crates/remote-exec-host/src crates/remote-exec-broker/tests
git commit -m "fix: track daemon port forward tasks"
```

### Task 13: Tighten Transfer Path Typing And Sandbox Absoluteness

**Findings:** `#18`, `#19`

**Files:**
- Modify: `crates/remote-exec-host/src/transfer/archive/mod.rs`
- Modify: `crates/remote-exec-host/src/transfer/archive/export.rs`
- Modify: `crates/remote-exec-host/src/transfer/archive/import.rs`
- Modify: `crates/remote-exec-host/src/transfer/mod.rs`
- Modify: `crates/remote-exec-proto/src/sandbox.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto sandbox`
  - `cargo test -p remote-exec-host transfer`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`

**Testing approach:** TDD for sandbox error variant + existing transfer tests
Reason: This changes validation responsibility. A failing test should first prove `authorize_path` reports non-absolute paths distinctly.

- [ ] **Step 1: Add a failing sandbox test.**

In `crates/remote-exec-proto/src/sandbox.rs`, add a unit test:

```rust
#[test]
fn authorize_path_rejects_relative_path_with_distinct_error() {
    let sandbox = CompiledFilesystemSandbox::default();
    let err = authorize_path(
        crate::path::PathPolicy::Posix,
        &sandbox,
        SandboxAccess::Read,
        std::path::Path::new("relative/path"),
    )
    .expect_err("relative path should be rejected");
    assert!(matches!(err, SandboxError::NotAbsolute { .. }));
}
```

Run: `cargo test -p remote-exec-proto authorize_path_rejects_relative_path_with_distinct_error`
Expected: fails before implementation.

- [ ] **Step 2: Add `SandboxError::NotAbsolute`.**

Update `SandboxError` and `authorize_path` so non-absolute paths return `NotAbsolute` instead of a generic validation error.

- [ ] **Step 3: Change `BundledArchiveSource.source_path` to `PathBuf`.**

Change:

```rust
pub source_path: String,
```

to:

```rust
pub source_path: PathBuf,
```

Normalize at construction time using the existing host path resolver. All later archive code should consume a typed path.

- [ ] **Step 4: Remove duplicated pre-checks.**

Delete explicit `is_input_path_absolute` checks in transfer import/export paths when they are immediately followed by `authorize_path`. Let `authorize_path` be the single source of truth and map `SandboxError::NotAbsolute` into the existing user-facing transfer error message.

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-proto sandbox
cargo test -p remote-exec-host transfer
cargo test -p remote-exec-daemon --test transfer_rpc
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-proto/src/sandbox.rs crates/remote-exec-host/src/transfer
git commit -m "refactor: centralize transfer path validation"
```

### Task 14: Precompute Daemon Bearer Authorization Header

**Findings:** `#17`

**Files:**
- Modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Modify: `crates/remote-exec-daemon/src/http/auth.rs`
- Modify: config tests if the auth struct shape changes
- Test/Verify:
  - `cargo test -p remote-exec-daemon --test health`
  - `cargo test -p remote-exec-daemon --lib config`

**Testing approach:** existing tests
Reason: This is allocation cleanup with no behavior change.

- [ ] **Step 1: Add a preformatted field to auth config.**

If the config struct currently is:

```rust
pub struct HttpAuthConfig {
    pub bearer_token: String,
}
```

change it to:

```rust
pub struct HttpAuthConfig {
    pub bearer_token: String,
    #[serde(skip)]
    pub expected_authorization: String,
}
```

If deriving deserialize directly makes this awkward, keep the serialized config struct as-is and add a runtime auth config type during validation.

- [ ] **Step 2: Populate after load/validation.**

After config load, set:

```rust
http_auth.expected_authorization = format!("Bearer {}", http_auth.bearer_token);
```

Do this once per loaded config.

- [ ] **Step 3: Use the precomputed value in auth middleware.**

Replace:

```rust
let expected = format!("Bearer {}", http_auth.bearer_token);
```

with:

```rust
let expected = http_auth.expected_authorization.as_str();
```

- [ ] **Step 4: Run verification.**

Run:

```bash
cargo test -p remote-exec-daemon --test health
cargo test -p remote-exec-daemon --lib config
```

Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon/src/config/mod.rs crates/remote-exec-daemon/src/http/auth.rs crates/remote-exec-daemon/src/config/tests.rs
git commit -m "refactor: precompute bearer auth header"
```

### Task 15: Finish C++ Path Utility Consolidation

**Findings:** partially addressed `#22`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/path_policy.h`
- Modify: `crates/remote-exec-daemon-cpp/src/path_policy.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/filesystem_sandbox.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/patch_engine.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing C++ tests
Reason: This is mechanical helper extraction and path normalization must remain byte-for-byte behavior compatible.

- [ ] **Step 1: Expose comparison helpers from `path_policy`.**

In `path_policy.h`, add:

```cpp
std::string path_policy_lowercase_ascii(std::string value);
std::string path_policy_comparison_key(PathPolicy policy, const std::string& raw);
```

Implement by renaming the private `lowercase_ascii` and `comparison_key` in `path_policy.cpp`. Update internal callers to use the exported names.

- [ ] **Step 2: Remove sandbox-local duplicates.**

In `filesystem_sandbox.cpp`, delete private `lowercase_ascii` and `comparison_key`, and call `path_policy_comparison_key`.

- [ ] **Step 3: Collapse patch relative/absolute normalization loops.**

Replace the two loops with:

```cpp
enum NormalizedPathKind {
    NORMALIZED_RELATIVE_PATH,
    NORMALIZED_ABSOLUTE_PATH
};

std::string normalize_path_segments(const std::string& raw, NormalizedPathKind kind) {
    // Preserve current separator handling, "." removal, ".." behavior,
    // and absolute-prefix behavior from normalize_relative_path and
    // normalize_absolute_path.
}
```

Keep `normalize_relative_path` and `normalize_absolute_path` as thin wrappers over the shared helper so callers do not change.

- [ ] **Step 4: Run verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/path_policy.h crates/remote-exec-daemon-cpp/src/path_policy.cpp crates/remote-exec-daemon-cpp/src/filesystem_sandbox.cpp crates/remote-exec-daemon-cpp/src/patch_engine.cpp
git commit -m "refactor: finish cpp path utility consolidation"
```

### Task 16: Finish C++ Thread RAII And Error Logging

**Findings:** partially addressed `#23`, `#24`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_runtime.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_runtime.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/connection_manager.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing tests + compile check
Reason: C++ RAII changes should preserve behavior; logging catch paths improves diagnostics without changing protocol.

- [ ] **Step 1: Replace raw thread pointers with `std::unique_ptr<std::thread>`.**

Use `std::unique_ptr<std::thread>` for:
- `PortTunnelService::expiry_thread_`
- `ServerRuntime::accept_thread_`
- `ServerRuntime::maintenance_thread_`
- `ConnectionRecord::thread`

Join through a local `std::unique_ptr<std::thread>` moved out under lock:

```cpp
std::unique_ptr<std::thread> thread;
{
    BasicLock lock(&mutex_);
    thread.swap(expiry_thread_);
}
if (thread.get() != NULL && thread->joinable()) {
    thread->join();
}
```

- [ ] **Step 2: Add catch logging helper.**

Create a local helper in each file or a shared helper if a logging utility already exists:

```cpp
bool log_unhandled_tunnel_error(const char* operation) {
    log_error(std::string(operation) + " failed with an unknown exception");
    return false;
}
```

Replace silent catch blocks:

```cpp
} catch (...) {
    service->release_worker();
    return false;
}
```

with:

```cpp
} catch (const std::exception& err) {
    log_error(std::string("... failed: ") + err.what());
    service->release_worker();
    return false;
} catch (...) {
    log_error("... failed with an unknown exception");
    service->release_worker();
    return false;
}
```

- [ ] **Step 3: Collapse Win32 tunnel context structs if it remains low risk.**

If `TcpReadContext`, `TcpWriteContext`, and `UdpReadContext` are still structurally identical, introduce:

```cpp
template <typename Operation>
struct WorkerContext {
    std::shared_ptr<PortTunnelConnection> connection;
    uint32_t stream_id;
    Operation operation;
};
```

If template use would compromise C++11/XP readability, leave the structs and only apply RAII/logging in this task.

- [ ] **Step 4: Run verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp crates/remote-exec-daemon-cpp/src/server_runtime.h crates/remote-exec-daemon-cpp/src/server_runtime.cpp crates/remote-exec-daemon-cpp/src/connection_manager.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_tcp.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_udp.cpp
git commit -m "refactor: finish cpp thread ownership cleanup"
```

### Task 17: Align C++ XP Test Standard And BSD Make Link Rules

**Findings:** `#27`, `#28`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/mk/common.mk`
- Modify: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`
  - `bmake -C crates/remote-exec-daemon-cpp check-posix` if `bmake` is installed

**Testing approach:** build-system verification
Reason: This is build parity cleanup. The useful test is compiling through all supported make paths.

- [ ] **Step 1: Split host and XP test standards.**

Set:

```make
HOST_TEST_CXXFLAGS := -std=gnu++17 -O0 -Wall -Wextra
XP_TEST_CXXFLAGS := -std=c++11 -O0 -Wall -Wextra
```

Use `XP_TEST_CXXFLAGS` for `WINDOWS_XP_TEST_CXXFLAGS` instead of inheriting `TEST_CXXFLAGS`.

- [ ] **Step 2: Refactor BSD make repeated link rules.**

Use a `.for` loop mapping test target variables to object variables. BSD make syntax should stay compatible:

```make
.for target objs in \
    ${HOST_PATCH} "${HOST_PATCH_OBJS}" \
    ${HOST_TRANSFER} "${HOST_TRANSFER_OBJS}"
${target}: ${objs}
	${HOST_CXX} ${TEST_CXXFLAGS} ${TEST_LDFLAGS} -o ${.TARGET} ${.ALLSRC} ${PTHREAD_LDLIBS}
.endfor
```

If quoting object lists in `.for` is not portable enough, use a small `.include`-style generated variable list checked into the Makefile, but keep one link recipe.

- [ ] **Step 3: Run verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
if command -v bmake >/dev/null 2>&1; then bmake -C crates/remote-exec-daemon-cpp check-posix; fi
```

Expected: GNU POSIX and XP checks pass. BSD make path passes when `bmake` is installed; otherwise record that `bmake` was unavailable.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/mk/common.mk crates/remote-exec-daemon-cpp/mk/windows-xp.mk crates/remote-exec-daemon-cpp/Makefile
git commit -m "build: align cpp test make paths"
```

### Task 18: Harden PKI Key Material Ownership

**Findings:** `#30`

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/remote-exec-pki/Cargo.toml`
- Modify: `crates/remote-exec-pki/src/generate.rs`
- Modify: `crates/remote-exec-pki/src/lib.rs`
- Modify: `crates/remote-exec-pki/src/write.rs`
- Modify: `crates/remote-exec-admin/src/certs.rs`
- Test/Verify:
  - `cargo test -p remote-exec-pki`
  - `cargo test -p remote-exec-admin --test dev_init`
  - `cargo test -p remote-exec-admin --test certs_issue`

**Testing approach:** existing tests + compile-time API tightening
Reason: This changes key material ownership. The important behavior is that admin can still generate/write certs while fewer callers can clone CA private key material directly.

- [ ] **Step 1: Add `zeroize`.**

Add a workspace dependency:

```toml
zeroize = { version = "1", features = ["zeroize_derive"] }
```

Add it to `remote-exec-pki` dependencies.

- [ ] **Step 2: Introduce `PrivateKeyPem`.**

In `generate.rs`:

```rust
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct PrivateKeyPem(Zeroizing<String>);

impl PrivateKeyPem {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Clone for PrivateKeyPem {
    fn clone(&self) -> Self {
        Self::new(self.as_str().to_string())
    }
}
```

Change:

```rust
pub key_pem: String,
```

to:

```rust
pub key_pem: PrivateKeyPem,
```

- [ ] **Step 3: Make CA key pair private.**

Change:

```rust
pub struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    pub pem_pair: GeneratedPemPair,
}
```

to:

```rust
pub struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    pem_pair: GeneratedPemPair,
}

impl CertificateAuthority {
    pub fn cert_pem(&self) -> &str {
        &self.pem_pair.cert_pem
    }

    pub fn key_pem(&self) -> &PrivateKeyPem {
        &self.pem_pair.key_pem
    }

    pub fn pem_pair(&self) -> &GeneratedPemPair {
        &self.pem_pair
    }
}
```

Use methods in `remote-exec-admin` instead of `ca.pem_pair`.

- [ ] **Step 4: Move issuance methods onto `CertificateAuthority`.**

Add:

```rust
impl CertificateAuthority {
    pub fn issue_broker_cert(&self, common_name: &str) -> anyhow::Result<GeneratedPemPair> {
        generate_broker_cert(self, common_name)
    }

    pub fn issue_daemon_cert(&self, daemon: &DaemonCertSpec) -> anyhow::Result<GeneratedPemPair> {
        generate_daemon_cert(self, daemon)
    }
}
```

Then make the free functions private if no external callers need them.

- [ ] **Step 5: Update writers for `PrivateKeyPem`.**

In `write.rs`, change key writes from `&pair.key_pem` to `pair.key_pem.as_str()`.

- [ ] **Step 6: Run verification.**

Run:

```bash
cargo test -p remote-exec-pki
cargo test -p remote-exec-admin --test dev_init
cargo test -p remote-exec-admin --test certs_issue
```

Expected: all pass.

- [ ] **Step 7: Commit.**

```bash
git add Cargo.toml Cargo.lock crates/remote-exec-pki/Cargo.toml crates/remote-exec-pki/src crates/remote-exec-admin/src/certs.rs
git commit -m "refactor: zeroize pki private key pem"
```

### Task 19: Fix PKI Atomic Write Semantics

**Findings:** `#31`

**Files:**
- Modify: `crates/remote-exec-pki/src/write.rs`
- Test/Verify:
  - `cargo test -p remote-exec-pki`
  - `cargo test -p remote-exec-admin --test dev_init`

**Testing approach:** TDD for write failure/permission behavior
Reason: Atomic file semantics need direct tests. Existing admin tests cover successful writes but not the failure windows.

- [ ] **Step 1: Add tests for no remove-before-rename and Unix permissions.**

In `write.rs` tests, add:

```rust
#[test]
fn write_text_file_does_not_remove_existing_file_before_successful_replace() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.key");
    std::fs::write(&path, "old").expect("old file");
    let mut written = Vec::new();
    write_text_file(&path, "new", true, 0o600, &mut written).expect("replace file");
    assert_eq!(std::fs::read_to_string(&path).expect("read file"), "new");
}

#[cfg(unix)]
#[test]
fn write_text_file_sets_key_permissions_after_rename() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ca.key");
    let mut written = Vec::new();
    write_text_file(&path, "secret", false, 0o600, &mut written).expect("write file");
    let mode = std::fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
```

- [ ] **Step 2: Use atomic rename without pre-removing the destination.**

Remove:

```rust
if path.exists() {
    fs::remove_file(path)?;
}
```

Use `fs::rename(&tmp_path, path)` directly on platforms where it replaces files. If Windows behavior requires explicit replacement, use platform-specific `std::fs::rename` semantics carefully and keep the old file until the new file is fully written.

- [ ] **Step 3: Set permissions after rename on Unix.**

On Unix:

```rust
fs::rename(&tmp_path, path)?;
fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
```

If setting permissions fails after rename, return the error with context. The file exists, but the caller sees that permission hardening failed.

- [ ] **Step 4: Document non-Unix behavior in code.**

Rename `_mode` to `mode`. On non-Unix, explicitly bind it:

```rust
#[cfg(not(unix))]
let _ = mode;
```

Add a short comment that Windows ACL hardening is not implemented by this crate yet.

- [ ] **Step 5: Run verification.**

Run:

```bash
cargo test -p remote-exec-pki
cargo test -p remote-exec-admin --test dev_init
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-pki/src/write.rs
git commit -m "fix: harden pki file writes"
```

### Task 20: Clean Admin PKI Manifest Placeholders And CLI Flag Drift

**Findings:** partially addressed `#33`, `#35`

**Files:**
- Modify: `crates/remote-exec-pki/src/manifest.rs`
- Modify: `crates/remote-exec-admin/src/cli.rs`
- Modify: `crates/remote-exec-admin/src/certs.rs`
- Modify: `README.md` if CLI docs mention these flags
- Test/Verify:
  - `cargo test -p remote-exec-pki`
  - `cargo test -p remote-exec-admin --test dev_init`
  - `cargo test -p remote-exec-admin --test certs_issue`

**Testing approach:** existing snapshot/parse tests + CLI tests
Reason: This affects generated operator text and CLI compatibility.

- [ ] **Step 1: Emit placeholder config values as comments.**

In broker snippet generation, change live placeholder:

```toml
base_url = "https://builder-a.example.com:9443"
```

to a comment:

```toml
# Set this to the daemon HTTPS endpoint.
# base_url = "https://builder-a.example.com:9443"
```

In daemon snippet generation, change live placeholder:

```toml
listen = "0.0.0.0:9443"
```

to:

```toml
# Set this to the daemon bind address.
# listen = "0.0.0.0:9443"
```

Ensure the snippet tests parse the uncommented TLS/path sections or parse only the intended TOML body.

- [ ] **Step 2: Standardize daemon SAN flag while preserving compatibility.**

For `IssueDaemonArgs`, keep `--san` as the primary flag. For `DevInitArgs`, accept both `--san` and `--daemon-san` if clap supports aliases in the current version:

```rust
#[arg(long = "san", visible_alias = "daemon-san")]
pub daemon_sans: Vec<String>,
```

If `visible_alias` is unsupported, use `alias = "daemon-san"` and update help text accordingly.

- [ ] **Step 3: Update parse errors and docs.**

Update strings that say `--daemon-san` so the canonical spelling is `--san`, while accepting the old spelling.

- [ ] **Step 4: Run verification.**

Run:

```bash
cargo test -p remote-exec-pki
cargo test -p remote-exec-admin --test dev_init
cargo test -p remote-exec-admin --test certs_issue
```

Expected: all pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-pki/src/manifest.rs crates/remote-exec-admin/src README.md
git commit -m "docs: clean pki manifest and san flags"
```

### Task 21: Final Cross-Workspace Verification

**Findings:** all

**Files:**
- Verify full workspace
- Modify only if final checks expose formatting, lint, or docs drift

**Testing approach:** full quality gate
Reason: Previous tasks are intentionally focused. The final task catches cross-crate drift and feature interactions.

- [ ] **Step 1: Run Rust formatting and tests.**

Run:

```bash
cargo fmt --all --check
cargo test --workspace
```

Expected: both pass.

- [ ] **Step 2: Run Rust clippy.**

Run:

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: passes.

- [ ] **Step 3: Run C++ checks.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
if command -v bmake >/dev/null 2>&1; then bmake -C crates/remote-exec-daemon-cpp check-posix; fi
```

Expected: GNU make checks pass. BSD make check passes when `bmake` is installed; otherwise record unavailable.

- [ ] **Step 4: Run diff hygiene checks.**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors. Status is clean after previous task commits, or only final verification fixes are present.

- [ ] **Step 5: Commit any final mechanical fixes.**

Only run this if Step 1-4 required changes:

```bash
git add .
git commit -m "chore: finish code audit remediation checks"
```

If there were no changes, do not create an empty commit.

---

## Execution Notes

- Use plan-based execution unless the user explicitly switches style; the user previously asked for direct plan-based execution and commits after each task.
- Do not batch unrelated task commits. Each task above is intended to leave the tree compiling and targeted tests passing.
- Do not rewrite `docs/CODE_AUDIT.md`; it is historical review context. If a finding becomes stale during execution, note that in the task result rather than editing the audit.
- Keep public wire strings stable unless a task explicitly says otherwise. Typed Rust/C++ enums should serialize to the same snake-case values currently used on the wire.
- For C++ changes, preserve C++11 and Windows XP constraints even when host POSIX tests compile with a newer standard.
