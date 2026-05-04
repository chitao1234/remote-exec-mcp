# Cross-Language Codebase Quality Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Execute a semi-aggressive cross-language refactor that reduces broker dispatch duplication, localizes transfer transport codec logic, splits the C++ route and port-forward monoliths, and centralizes test support, while keeping the public MCP tool schemas and `exec_command` patch interception behavior stable.

**Architecture:** Clean the broker control plane first, then extract shared test scaffolding, then localize transfer transport logic in Rust, then thin the Rust daemon HTTP shell, then split the C++ route and port-forward modules. Keep public MCP schemas stable; allow internal module, helper, and build-graph changes as long as behavior and tests remain stable.

**Tech Stack:** Rust 2024, Tokio, axum, reqwest, serde, rmcp, existing broker and daemon integration tests, GNU make, g++/MinGW C++17 build, existing C++ host tests, cargo test, cargo fmt, cargo clippy

---

### Task 1: Split broker runtime state, startup, and target modules

**Files:**
- Create: `crates/remote-exec-broker/src/state.rs`
- Create: `crates/remote-exec-broker/src/startup.rs`
- Create: `crates/remote-exec-broker/src/target/mod.rs`
- Create: `crates/remote-exec-broker/src/target/backend.rs`
- Create: `crates/remote-exec-broker/src/target/handle.rs`
- Modify: `crates/remote-exec-broker/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Test/Verify: `cargo test -p remote-exec-broker --lib`

**Testing approach:** `existing tests + targeted verification`
Reason: this slice is an internal broker layout refactor. The broker unit tests and startup helpers are the fastest proof that the extracted modules still compile and preserve existing startup behavior.

- [ ] **Step 1: Add the new broker module scaffolding and re-exports**

```rust
// crates/remote-exec-broker/src/lib.rs
pub(crate) mod broker_tls;
pub mod client;
pub mod config;
pub mod daemon_client;
pub mod local_backend;
pub mod local_transfer;
pub mod logging;
pub mod mcp_server;
pub mod port_forward;
pub mod session_store;
pub mod startup;
pub mod state;
pub mod target;
pub mod tools;

pub use startup::{build_state, run};
pub use state::BrokerState;
pub use target::{CachedDaemonInfo, TargetHandle};
```

- [ ] **Step 2: Move broker state and startup assembly into focused modules**

```rust
// crates/remote-exec-broker/src/state.rs
use std::collections::BTreeMap;

use remote_exec_proto::sandbox::CompiledFilesystemSandbox;

use crate::{port_forward, session_store::SessionStore, target::TargetHandle};

#[derive(Clone)]
pub struct BrokerState {
    pub enable_transfer_compression: bool,
    pub disable_structured_content: bool,
    pub host_sandbox: Option<CompiledFilesystemSandbox>,
    pub sessions: SessionStore,
    pub port_forwards: port_forward::PortForwardStore,
    pub targets: BTreeMap<String, TargetHandle>,
}

impl BrokerState {
    pub fn target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unknown target `{name}`"))
    }
}

// crates/remote-exec-broker/src/startup.rs
pub async fn run(config: crate::config::BrokerConfig) -> anyhow::Result<()> {
    crate::install_crypto_provider();
    let mcp = config.mcp.clone();
    let state = build_state(config).await?;
    crate::mcp_server::serve(state, &mcp).await
}
```

- [ ] **Step 3: Move target-handle logic out of `lib.rs` and keep only target-focused code in the new module**

```rust
// crates/remote-exec-broker/src/target/mod.rs
mod backend;
mod handle;

pub use handle::{CachedDaemonInfo, TargetHandle};

// crates/remote-exec-broker/src/target/backend.rs
#[derive(Clone)]
pub(crate) enum TargetBackend {
    Remote(crate::daemon_client::DaemonClient),
    Local(crate::local_backend::LocalDaemonClient),
}

// crates/remote-exec-broker/src/target/handle.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedDaemonInfo {
    pub daemon_version: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_transfer_compression: bool,
    pub supports_port_forward: bool,
}
```

- [ ] **Step 4: Run the focused broker verification**

Run: `cargo test -p remote-exec-broker --lib`
Expected: PASS with the broker startup and state unit tests still green.

- [ ] **Step 5: Commit the broker module split**

```bash
git add \
  crates/remote-exec-broker/src/lib.rs \
  crates/remote-exec-broker/src/state.rs \
  crates/remote-exec-broker/src/startup.rs \
  crates/remote-exec-broker/src/target/mod.rs \
  crates/remote-exec-broker/src/target/backend.rs \
  crates/remote-exec-broker/src/target/handle.rs \
  crates/remote-exec-broker/src/tools/targets.rs
git commit -m "refactor: split broker state and target modules"
```

### Task 2: Centralize broker backend capability dispatch and reuse it in port forwarding

**Files:**
- Create: `crates/remote-exec-broker/src/target/capabilities.rs`
- Create: `crates/remote-exec-broker/src/local_port_backend.rs`
- Modify: `crates/remote-exec-broker/src/target/mod.rs`
- Modify: `crates/remote-exec-broker/src/target/backend.rs`
- Modify: `crates/remote-exec-broker/src/target/handle.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify: `crates/remote-exec-broker/src/tools/patch.rs`
- Modify: `crates/remote-exec-broker/src/tools/image.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Modify: `crates/remote-exec-broker/src/lib.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_exec`, `cargo test -p remote-exec-broker --test mcp_assets`, `cargo test -p remote-exec-broker --test mcp_forward_ports`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** `existing tests + targeted verification`
Reason: this refactor changes how broker tool handlers and port forwarding reach local and remote targets. The focused broker MCP suites are the right proof that public behavior remains stable.

- [ ] **Step 1: Introduce focused capability helpers on the target module instead of repeating tool-local transport handling**

```rust
// crates/remote-exec-broker/src/target/capabilities.rs
use remote_exec_proto::rpc::{
    EmptyResponse, ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, PortConnectRequest,
    PortConnectResponse, PortConnectionCloseRequest, PortConnectionReadRequest,
    PortConnectionReadResponse, PortConnectionWriteRequest, PortListenAcceptRequest,
    PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest, PortListenResponse,
    PortUdpDatagramReadRequest, PortUdpDatagramReadResponse, PortUdpDatagramWriteRequest,
    TransferExportRequest, TransferExportResponse, TransferImportRequest, TransferImportResponse,
    TransferPathInfoRequest, TransferPathInfoResponse,
};

impl crate::TargetHandle {
    pub async fn exec_start_checked(
        &self,
        target_name: &str,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, crate::daemon_client::DaemonClientError> {
        self.ensure_identity_verified(target_name).await?;
        self.backend.exec_start(req).await
    }

    pub async fn image_read_checked(
        &self,
        target_name: &str,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, crate::daemon_client::DaemonClientError> {
        self.ensure_identity_verified(target_name).await?;
        self.backend.image_read(req).await
    }

    pub async fn clear_on_transport_error<T>(
        &self,
        result: Result<T, crate::daemon_client::DaemonClientError>,
    ) -> Result<T, crate::daemon_client::DaemonClientError> {
        if matches!(result, Err(crate::daemon_client::DaemonClientError::Transport(_))) {
            self.clear_cached_daemon_info().await;
        }
        result
    }
}
```

- [ ] **Step 2: Move the broker-local port-forward runtime out of `port_forward.rs` into its own backend module**

```rust
// crates/remote-exec-broker/src/local_port_backend.rs
#[derive(Clone)]
pub struct LocalPortClient {
    state: std::sync::Arc<remote_exec_host::HostRuntimeState>,
}

impl LocalPortClient {
    pub fn global() -> Self {
        static STATE: std::sync::OnceLock<std::sync::Arc<remote_exec_host::HostRuntimeState>> =
            std::sync::OnceLock::new();
        let state = STATE
            .get_or_init(|| {
                let config = remote_exec_host::EmbeddedHostConfig {
                    target: "local".to_string(),
                    default_workdir: std::env::current_dir()
                        .unwrap_or_else(|_| std::env::temp_dir()),
                    windows_posix_root: None,
                    sandbox: None,
                    enable_transfer_compression: false,
                    allow_login_shell: false,
                    pty: remote_exec_host::PtyMode::None,
                    default_shell: None,
                    yield_time: remote_exec_host::YieldTimeConfig::default(),
                    experimental_apply_patch_target_encoding_autodetect: false,
                    process_environment: remote_exec_host::ProcessEnvironment::capture_current(),
                };
                std::sync::Arc::new(
                    remote_exec_host::build_runtime_state(config.into_host_runtime_config())
                        .expect("construct local port runtime"),
                )
            })
            .clone();
        Self { state }
    }
}
```

- [ ] **Step 3: Rewrite the broker tool handlers and `SideHandle` to use the shared helpers**

```rust
// crates/remote-exec-broker/src/tools/image.rs
let target = state.target(&input.target)?;
let response = target
    .clear_on_transport_error(target.image_read_checked(&input.target, &ImageReadRequest {
        path: input.path,
        workdir: input.workdir,
        detail: input.detail.clone(),
    }).await)
    .await?;

// crates/remote-exec-broker/src/port_forward.rs
#[derive(Clone)]
pub enum SideHandle {
    Target { name: String, handle: crate::TargetHandle },
    Local(crate::local_port_backend::LocalPortClient),
}
```

- [ ] **Step 4: Run the focused broker MCP suites**

Run: `cargo test -p remote-exec-broker --test mcp_exec`
Expected: PASS with `exec_command`, `write_stdin`, and patch interception behavior unchanged.

Run: `cargo test -p remote-exec-broker --test mcp_assets`
Expected: PASS with `view_image` behavior unchanged.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: PASS with local and remote port-forward operations unchanged.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS with target isolation and broker-local target behavior unchanged.

- [ ] **Step 5: Commit the broker capability slice**

```bash
git add \
  crates/remote-exec-broker/src/lib.rs \
  crates/remote-exec-broker/src/local_port_backend.rs \
  crates/remote-exec-broker/src/port_forward.rs \
  crates/remote-exec-broker/src/target/mod.rs \
  crates/remote-exec-broker/src/target/backend.rs \
  crates/remote-exec-broker/src/target/capabilities.rs \
  crates/remote-exec-broker/src/target/handle.rs \
  crates/remote-exec-broker/src/tools/exec.rs \
  crates/remote-exec-broker/src/tools/patch.rs \
  crates/remote-exec-broker/src/tools/image.rs \
  crates/remote-exec-broker/src/tools/transfer/endpoints.rs
git commit -m "refactor: centralize broker backend dispatch"
```

### Task 3: Extract shared transfer test support and slim stub-daemon helpers

**Files:**
- Create: `tests/support/transfer_archive.rs`
- Create: `crates/remote-exec-broker/tests/support/stub_daemon_exec.rs`
- Create: `crates/remote-exec-broker/tests/support/stub_daemon_image.rs`
- Create: `crates/remote-exec-broker/tests/support/stub_daemon_transfer.rs`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Modify: `crates/remote-exec-daemon/tests/support/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test -p remote-exec-daemon --test transfer_rpc`

**Testing approach:** `existing tests + targeted verification`
Reason: the purpose here is maintainability, not behavior change. Focused transfer suites are enough to prove the shared helper extraction did not alter archive semantics.

- [ ] **Step 1: Add one workspace-level shared archive helper module and import it by path from both broker and daemon test support**

```rust
// tests/support/transfer_archive.rs
use std::io::{Cursor, Read};

pub fn decode_archive(bytes: &[u8], compression: &str) -> Vec<u8> {
    match compression {
        "zstd" => zstd::stream::decode_all(Cursor::new(bytes)).expect("decode zstd archive"),
        _ => bytes.to_vec(),
    }
}

pub fn read_archive_paths(bytes: &[u8], compression: &str) -> Vec<String> {
    let decoded = decode_archive(bytes, compression);
    let mut archive = tar::Archive::new(Cursor::new(decoded));
    archive
        .entries()
        .expect("archive entries")
        .map(|entry| {
            entry
                .expect("archive entry")
                .path()
                .expect("entry path")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}
```

- [ ] **Step 2: Break the broker stub daemon into feature slices instead of one monolith**

```rust
// crates/remote-exec-broker/tests/support/mod.rs
pub mod stub_daemon;

// crates/remote-exec-broker/tests/support/stub_daemon.rs
mod stub_daemon_exec;
mod stub_daemon_image;
mod stub_daemon_transfer;

pub use stub_daemon_exec::{
    set_exec_start_behavior,
    set_exec_write_behavior,
    ExecStartBehavior,
    ExecWriteBehavior,
};
pub use stub_daemon_image::{set_image_read_response, StubImageReadResponse};
pub use stub_daemon_transfer::{
    set_transfer_export_directory_response,
    set_transfer_export_file_response,
    set_transfer_path_info_response,
    StubTransferExportCapture,
    StubTransferImportCapture,
    StubTransferPathInfoResponse,
};
```

- [ ] **Step 3: Replace the duplicated local helper functions in the transfer suites with the shared module**

```rust
// crates/remote-exec-broker/tests/support/mod.rs
#[path = "../../../../tests/support/transfer_archive.rs"]
pub mod transfer_archive;

// crates/remote-exec-daemon/tests/support/mod.rs
#[path = "../../../../tests/support/transfer_archive.rs"]
pub mod transfer_archive;

// crates/remote-exec-broker/tests/mcp_transfer.rs
use support::transfer_archive::{decode_archive, read_archive_paths};

// crates/remote-exec-daemon/tests/transfer_rpc.rs
use support::transfer_archive::decode_archive;
```

- [ ] **Step 4: Run the focused transfer suites**

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: PASS with transfer summaries and archive assertions unchanged.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: PASS with raw transfer route behavior unchanged.

- [ ] **Step 5: Commit the shared test-harness extraction**

```bash
git add \
  tests/support/transfer_archive.rs \
  crates/remote-exec-broker/tests/support/mod.rs \
  crates/remote-exec-broker/tests/support/stub_daemon.rs \
  crates/remote-exec-broker/tests/support/stub_daemon_exec.rs \
  crates/remote-exec-broker/tests/support/stub_daemon_image.rs \
  crates/remote-exec-broker/tests/support/stub_daemon_transfer.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs \
  crates/remote-exec-daemon/tests/support/mod.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs
git commit -m "test: extract shared transfer support"
```

### Task 4: Localize Rust transfer transport codecs in proto, broker, and daemon layers

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Create: `crates/remote-exec-broker/src/tools/transfer/codec.rs`
- Create: `crates/remote-exec-daemon/src/transfer/codec.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/mod.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test -p remote-exec-daemon --test transfer_rpc`

**Testing approach:** `existing tests + targeted verification`
Reason: this refactor changes how transfer metadata is represented and parsed internally. The broker and daemon transfer suites are the right regression surface.

- [ ] **Step 1: Add shared metadata carrier types in `remote-exec-proto` without changing the wire field names or MCP schemas**

```rust
// crates/remote-exec-proto/src/rpc.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferExportMetadata {
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferImportMetadata {
    pub destination_path: String,
    pub overwrite: TransferOverwriteMode,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub symlink_mode: TransferSymlinkMode,
}

impl From<&TransferImportRequest> for TransferImportMetadata {
    fn from(value: &TransferImportRequest) -> Self {
        Self {
            destination_path: value.destination_path.clone(),
            overwrite: value.overwrite.clone(),
            create_parent: value.create_parent,
            source_type: value.source_type.clone(),
            compression: value.compression.clone(),
            symlink_mode: value.symlink_mode.clone(),
        }
    }
}
```

- [ ] **Step 2: Route broker transfer header formatting and parsing through a dedicated codec module**

```rust
// crates/remote-exec-broker/src/tools/transfer/codec.rs
use remote_exec_proto::rpc::{
    TransferExportMetadata, TransferImportMetadata, TRANSFER_COMPRESSION_HEADER,
    TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER, TRANSFER_SYMLINK_MODE_HEADER,
};

pub fn apply_import_headers(
    builder: reqwest::RequestBuilder,
    metadata: &TransferImportMetadata,
) -> reqwest::RequestBuilder {
    builder
        .header(TRANSFER_DESTINATION_PATH_HEADER, metadata.destination_path.clone())
        .header(TRANSFER_OVERWRITE_HEADER, match metadata.overwrite {
            remote_exec_proto::rpc::TransferOverwriteMode::Fail => "fail",
            remote_exec_proto::rpc::TransferOverwriteMode::Merge => "merge",
            remote_exec_proto::rpc::TransferOverwriteMode::Replace => "replace",
        })
        .header(TRANSFER_CREATE_PARENT_HEADER, metadata.create_parent.to_string())
        .header(TRANSFER_SOURCE_TYPE_HEADER, match metadata.source_type {
            remote_exec_proto::rpc::TransferSourceType::File => "file",
            remote_exec_proto::rpc::TransferSourceType::Directory => "directory",
            remote_exec_proto::rpc::TransferSourceType::Multiple => "multiple",
        })
        .header(TRANSFER_COMPRESSION_HEADER, match metadata.compression {
            remote_exec_proto::rpc::TransferCompression::None => "none",
            remote_exec_proto::rpc::TransferCompression::Zstd => "zstd",
        })
        .header(TRANSFER_SYMLINK_MODE_HEADER, match metadata.symlink_mode {
            remote_exec_proto::rpc::TransferSymlinkMode::Preserve => "preserve",
            remote_exec_proto::rpc::TransferSymlinkMode::Follow => "follow",
            remote_exec_proto::rpc::TransferSymlinkMode::Skip => "skip",
        })
}
```

- [ ] **Step 3: Route daemon transfer header parsing and response metadata shaping through its own codec module**

```rust
// crates/remote-exec-daemon/src/transfer/codec.rs
use axum::Json;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use remote_exec_proto::rpc::{RpcErrorBody, TransferImportMetadata, TransferImportRequest};

pub fn parse_import_metadata(
    headers: &HeaderMap,
) -> Result<TransferImportMetadata, (StatusCode, Json<RpcErrorBody>)> {
    Ok(TransferImportMetadata {
        destination_path: required_header_string(headers, remote_exec_proto::rpc::TRANSFER_DESTINATION_PATH_HEADER)?,
        overwrite: parse_required_header_enum(headers, remote_exec_proto::rpc::TRANSFER_OVERWRITE_HEADER)?,
        create_parent: required_header_string(headers, remote_exec_proto::rpc::TRANSFER_CREATE_PARENT_HEADER)?
            .parse::<bool>()
            .map_err(|err| crate::rpc_error::bad_request(format!("invalid create_parent header: {err}")))?,
        source_type: parse_required_header_enum(headers, remote_exec_proto::rpc::TRANSFER_SOURCE_TYPE_HEADER)?,
        compression: parse_optional_header_enum(headers, remote_exec_proto::rpc::TRANSFER_COMPRESSION_HEADER)?
            .unwrap_or_default(),
        symlink_mode: parse_optional_header_enum(headers, remote_exec_proto::rpc::TRANSFER_SYMLINK_MODE_HEADER)?
            .unwrap_or_default(),
    })
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

- [ ] **Step 4: Run the focused transfer regression suites**

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: PASS with broker transfer import/export behavior and summaries unchanged.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: PASS with daemon header parsing and export headers unchanged from the client perspective.

- [ ] **Step 5: Commit the Rust transfer codec cleanup**

```bash
git add \
  crates/remote-exec-proto/src/rpc.rs \
  crates/remote-exec-broker/src/tools/transfer/mod.rs \
  crates/remote-exec-broker/src/tools/transfer/codec.rs \
  crates/remote-exec-broker/src/daemon_client.rs \
  crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/transfer/codec.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs
git commit -m "refactor: isolate rust transfer transport codecs"
```

### Task 5: Thin the Rust daemon HTTP shell and move middleware and route assembly into focused modules

**Files:**
- Create: `crates/remote-exec-daemon/src/http/mod.rs`
- Create: `crates/remote-exec-daemon/src/http/auth.rs`
- Create: `crates/remote-exec-daemon/src/http/request_log.rs`
- Create: `crates/remote-exec-daemon/src/http/routes.rs`
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Modify: `crates/remote-exec-daemon/src/main.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test health`, `cargo test -p remote-exec-daemon --test exec_rpc`, `cargo test -p remote-exec-daemon --test patch_rpc`, `cargo test -p remote-exec-daemon --test image_rpc`, `cargo test -p remote-exec-daemon --test transfer_rpc`, `cargo test -p remote-exec-daemon --test port_forward_rpc`

**Testing approach:** `existing tests + targeted verification`
Reason: the daemon public HTTP behavior must stay unchanged while the route/middleware wiring moves. The route-level test suite is the right proof.

- [ ] **Step 1: Create a dedicated daemon HTTP module for auth, request logging, and route wiring**

```rust
// crates/remote-exec-daemon/src/http/mod.rs
pub mod auth;
pub mod request_log;
pub mod routes;

// crates/remote-exec-daemon/src/http/routes.rs
pub fn router(
    state: std::sync::Arc<crate::AppState>,
    daemon_config: std::sync::Arc<crate::config::DaemonConfig>,
) -> axum::Router {
    axum::Router::new()
        .route("/v1/health", axum::routing::post(super::super::server::health))
        .route("/v1/target-info", axum::routing::post(super::super::server::target_info))
        .route("/v1/exec/start", axum::routing::post(crate::exec::exec_start))
        .route("/v1/exec/write", axum::routing::post(crate::exec::exec_write))
        .route("/v1/patch/apply", axum::routing::post(crate::patch::apply_patch))
        .route("/v1/transfer/path-info", axum::routing::post(crate::transfer::path_info))
        .route("/v1/transfer/export", axum::routing::post(crate::transfer::export_path))
        .route("/v1/transfer/import", axum::routing::post(crate::transfer::import_archive))
        .route("/v1/image/read", axum::routing::post(crate::image::read_image))
        .route("/v1/port/listen", axum::routing::post(crate::port_forward::listen))
        .layer(axum::middleware::from_fn_with_state(
            daemon_config,
            super::auth::require_http_auth,
        ))
        .with_state(state)
        .layer(axum::middleware::from_fn(super::request_log::log_http_request))
}
```

- [ ] **Step 2: Reduce `server.rs` to serving concerns only and point it at the new HTTP router**

```rust
// crates/remote-exec-daemon/src/server.rs
pub async fn serve_with_shutdown<F>(
    state: crate::AppState,
    daemon_config: std::sync::Arc<crate::config::DaemonConfig>,
    shutdown: F,
) -> anyhow::Result<()>
where
    F: std::future::Future<Output = ()> + Send,
{
    let state = std::sync::Arc::new(state);
    let app = crate::http::routes::router(state, daemon_config.clone());
    crate::tls::serve_with_shutdown(app, daemon_config, shutdown).await
}
```

- [ ] **Step 3: Run the daemon route-focused verification set**

Run: `cargo test -p remote-exec-daemon --test health`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test exec_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test patch_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test image_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
Expected: PASS.

- [ ] **Step 4: Commit the daemon transport-shell refactor**

```bash
git add \
  crates/remote-exec-daemon/src/lib.rs \
  crates/remote-exec-daemon/src/main.rs \
  crates/remote-exec-daemon/src/server.rs \
  crates/remote-exec-daemon/src/http/mod.rs \
  crates/remote-exec-daemon/src/http/auth.rs \
  crates/remote-exec-daemon/src/http/request_log.rs \
  crates/remote-exec-daemon/src/http/routes.rs
git commit -m "refactor: thin daemon http shell"
```

### Task 6: Split C++ server routes by feature and add a dedicated transfer HTTP codec layer

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/server_route_common.h`
- Create: `crates/remote-exec-daemon-cpp/include/server_route_exec.h`
- Create: `crates/remote-exec-daemon-cpp/include/server_route_image.h`
- Create: `crates/remote-exec-daemon-cpp/include/server_route_port_forward.h`
- Create: `crates/remote-exec-daemon-cpp/include/server_route_transfer.h`
- Create: `crates/remote-exec-daemon-cpp/include/transfer_http_codec.h`
- Create: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/server_route_exec.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/server_route_image.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/server_route_port_forward.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server_routes.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-transfer`, `make -C crates/remote-exec-daemon-cpp test-host-server-routes`, `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** `existing tests + targeted verification`
Reason: the route split changes C++ file boundaries but not behavior. The existing route and streaming tests are the best proof that request dispatch remains correct.

- [ ] **Step 1: Add per-feature route headers and a transfer HTTP codec helper**

```cpp
// crates/remote-exec-daemon-cpp/include/server_route_exec.h
#pragma once

#include "http_request.h"
#include "server.h"

HttpResponse handle_exec_start(AppState& state, const HttpRequest& request);
HttpResponse handle_exec_write(AppState& state, const HttpRequest& request);

// crates/remote-exec-daemon-cpp/include/transfer_http_codec.h
#pragma once

#include <string>

#include "http_request.h"
#include "transfer_ops.h"

struct TransferImportMetadata {
    std::string destination_path;
    std::string overwrite;
    bool create_parent;
    std::string source_type;
    std::string compression;
    std::string symlink_mode;
};

TransferImportMetadata parse_transfer_import_metadata(const HttpRequest& request);
void write_transfer_export_headers(HttpResponse& response, const ExportedPayload& payload);
```

- [ ] **Step 2: Turn `server_routes.cpp` into a thin dispatcher and move transfer/image/exec/port-forward bodies into the new files**

```cpp
// crates/remote-exec-daemon-cpp/src/server_routes.cpp
#include "server_route_exec.h"
#include "server_route_image.h"
#include "server_route_port_forward.h"
#include "server_route_transfer.h"

HttpResponse route_request(AppState& state, const HttpRequest& request) {
    if (!state.config.http_auth_bearer_token.empty() &&
        !request_has_bearer_auth(request, state.config.http_auth_bearer_token)) {
        HttpResponse response;
        write_bearer_auth_challenge(response);
        return response;
    }

    if (request.method != "POST") {
        return make_rpc_error_response(405, "method_not_allowed", "only POST is supported");
    }

    if (request.path == "/v1/exec/start") {
        return handle_exec_start(state, request);
    }
    if (request.path == "/v1/transfer/import") {
        return handle_transfer_import(state, request);
    }
    if (request.path == "/v1/port/listen") {
        return handle_port_listen(state, request);
    }
    return make_rpc_error_response(404, "not_found", "unknown endpoint");
}
```

- [ ] **Step 3: Update the Makefile source lists to build the new route units**

```make
# crates/remote-exec-daemon-cpp/Makefile
COMMON_SRCS := $(addprefix $(MAKEFILE_DIR), \
  src/main.cpp src/config.cpp src/http_helpers.cpp src/http_request.cpp src/logging.cpp \
  src/text_utils.cpp src/platform.cpp src/shell_policy.cpp src/server.cpp \
  src/server_routes.cpp src/server_route_common.cpp src/server_route_exec.cpp \
  src/server_route_image.cpp src/server_route_port_forward.cpp src/server_route_transfer.cpp \
  src/transfer_http_codec.cpp src/server_transport.cpp src/session_store.cpp \
  src/patch_engine.cpp src/port_forward.cpp src/basic_mutex.cpp) \
  $(TRANSFER_SRCS) $(POLICY_SRCS) $(RPC_FAILURE_SRCS)
```

- [ ] **Step 4: Run the focused C++ route verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS with route status codes and transfer metadata behavior unchanged.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS with end-to-end server streaming behavior unchanged.

- [ ] **Step 5: Commit the C++ route split**

```bash
git add \
  crates/remote-exec-daemon-cpp/include/server_routes.h \
  crates/remote-exec-daemon-cpp/include/server_route_common.h \
  crates/remote-exec-daemon-cpp/include/server_route_exec.h \
  crates/remote-exec-daemon-cpp/include/server_route_image.h \
  crates/remote-exec-daemon-cpp/include/server_route_port_forward.h \
  crates/remote-exec-daemon-cpp/include/server_route_transfer.h \
  crates/remote-exec-daemon-cpp/include/transfer_http_codec.h \
  crates/remote-exec-daemon-cpp/src/server_routes.cpp \
  crates/remote-exec-daemon-cpp/src/server_route_common.cpp \
  crates/remote-exec-daemon-cpp/src/server_route_exec.cpp \
  crates/remote-exec-daemon-cpp/src/server_route_image.cpp \
  crates/remote-exec-daemon-cpp/src/server_route_port_forward.cpp \
  crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp \
  crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp \
  crates/remote-exec-daemon-cpp/Makefile \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp \
  crates/remote-exec-daemon-cpp/tests/test_transfer.cpp
git commit -m "refactor: split cpp server routes by feature"
```

### Task 7: Split C++ port-forward internals into endpoint, codec, and socket helper modules

**Files:**
- Create: `crates/remote-exec-daemon-cpp/include/port_forward_endpoint.h`
- Create: `crates/remote-exec-daemon-cpp/include/port_forward_codec.h`
- Create: `crates/remote-exec-daemon-cpp/include/port_forward_socket_ops.h`
- Create: `crates/remote-exec-daemon-cpp/src/port_forward_endpoint.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/port_forward_codec.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/port_forward.h`
- Modify: `crates/remote-exec-daemon-cpp/src/port_forward.cpp`
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`, `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`, `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** `existing tests + targeted verification`
Reason: this is the biggest C++ structural split. The existing route and streaming tests plus the full POSIX check are the right safety net.

- [ ] **Step 1: Create explicit endpoint and codec helper headers**

```cpp
// crates/remote-exec-daemon-cpp/include/port_forward_endpoint.h
#pragma once

#include <string>

struct ParsedPortForwardEndpoint {
    std::string host;
    std::string port;
};

ParsedPortForwardEndpoint parse_port_forward_endpoint(const std::string& endpoint);
unsigned long parse_port_number(const std::string& value);
std::string normalize_port_forward_endpoint(const std::string& endpoint);
std::string ensure_nonzero_connect_endpoint(const std::string& endpoint);

// crates/remote-exec-daemon-cpp/include/port_forward_codec.h
#pragma once

#include <string>

std::string base64_encode_bytes(const std::string& bytes);
std::string base64_decode_bytes(const std::string& data);
```

- [ ] **Step 2: Move socket-level helpers out of `port_forward.cpp` and keep the store class focused on lifecycle orchestration**

```cpp
// crates/remote-exec-daemon-cpp/include/port_forward_socket_ops.h
#pragma once

#include <string>

#include "server_transport.h"

SOCKET bind_socket(const std::string& endpoint, const std::string& protocol);
SOCKET connect_socket(const std::string& endpoint, const std::string& protocol);
std::string socket_local_endpoint(SOCKET socket);
sockaddr_storage parse_peer_endpoint(const std::string& peer, socklen_t* peer_len);

// crates/remote-exec-daemon-cpp/src/port_forward.cpp
#include "port_forward_codec.h"
#include "port_forward_endpoint.h"
#include "port_forward_socket_ops.h"

Json PortForwardStore::listen(const std::string& endpoint, const std::string& protocol) {
    const std::string normalized = normalize_port_forward_endpoint(endpoint);
    UniqueSocket socket(bind_socket(normalized, protocol));
    const std::string bind_id = make_port_id("bind");
    const std::string actual_endpoint = socket_local_endpoint(socket.get());
    // Keep the existing store logic here; move parsing and socket setup out.
}
```

- [ ] **Step 3: Update the Makefile and keep the route tests using the new source units**

```make
# crates/remote-exec-daemon-cpp/Makefile
COMMON_SRCS := $(addprefix $(MAKEFILE_DIR), \
  src/main.cpp src/config.cpp src/http_helpers.cpp src/http_request.cpp src/logging.cpp \
  src/text_utils.cpp src/platform.cpp src/shell_policy.cpp src/server.cpp \
  src/server_routes.cpp src/server_route_common.cpp src/server_route_exec.cpp \
  src/server_route_image.cpp src/server_route_port_forward.cpp src/server_route_transfer.cpp \
  src/transfer_http_codec.cpp src/server_transport.cpp src/session_store.cpp \
  src/patch_engine.cpp src/port_forward.cpp src/port_forward_endpoint.cpp \
  src/port_forward_codec.cpp src/port_forward_socket_ops.cpp src/basic_mutex.cpp) \
  $(TRANSFER_SRCS) $(POLICY_SRCS) $(RPC_FAILURE_SRCS)
```

- [ ] **Step 4: Run the focused C++ port-forward verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS with all host-side C++ tests and the POSIX daemon build green.

- [ ] **Step 5: Commit the C++ port-forward split**

```bash
git add \
  crates/remote-exec-daemon-cpp/include/port_forward.h \
  crates/remote-exec-daemon-cpp/include/port_forward_endpoint.h \
  crates/remote-exec-daemon-cpp/include/port_forward_codec.h \
  crates/remote-exec-daemon-cpp/include/port_forward_socket_ops.h \
  crates/remote-exec-daemon-cpp/src/port_forward.cpp \
  crates/remote-exec-daemon-cpp/src/port_forward_endpoint.cpp \
  crates/remote-exec-daemon-cpp/src/port_forward_codec.cpp \
  crates/remote-exec-daemon-cpp/src/port_forward_socket_ops.cpp \
  crates/remote-exec-daemon-cpp/Makefile \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "refactor: split cpp port forward internals"
```

### Task 8: Run the full cross-language quality gate and land any final compatibility fixes

**Files:**
- Modify: any files required by gate-driven compatibility fixes
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** `existing tests + targeted verification`
Reason: this task proves the full broker + daemon + C++ refactor is integrated cleanly and still meets the repo’s quality gate.

- [ ] **Step 1: Run the Rust workspace gate**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 2: Run the C++ full POSIX gate**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS with all host-side C++ tests and the POSIX daemon build green.

- [ ] **Step 3: Fix any gate-discovered regressions immediately and rerun only the failing subset before rerunning the full gate**

```bash
# Example rerun loop if one suite fails during the full gate:
cargo test -p remote-exec-broker --test mcp_transfer
cargo test -p remote-exec-daemon --test transfer_rpc
make -C crates/remote-exec-daemon-cpp test-host-server-routes

# Then rerun the full gates:
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
make -C crates/remote-exec-daemon-cpp check-posix
```

- [ ] **Step 4: Commit the final gate-driven fixes and integration polish**

```bash
git add \
  crates/remote-exec-broker \
  crates/remote-exec-daemon \
  crates/remote-exec-proto \
  crates/remote-exec-daemon-cpp \
  tests/support
git commit -m "refactor: finish cross-language codebase cleanup"
```
