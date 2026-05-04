# Host Runtime Boundary And Error Classification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Extract a shared `remote-exec-host` runtime crate, remove the broker's production dependency on daemon internals, and replace transfer/image message-sniffing with explicit error categories in both daemon implementations without changing the public tool contract.

**Architecture:** Add a new internal Rust crate, `remote-exec-host`, that owns host runtime config, state, exec, patch, image, transfer, and port-forward logic. `remote-exec-daemon` becomes a transport wrapper over that runtime, while `remote-exec-broker` uses the same runtime only for broker-host `local` behavior and keeps remote-target behavior on HTTP RPC. Rust transfer/image failures move to typed internal errors mapped directly to RPC codes; the C++ daemon mirrors the same explicit error-code selection internally.

**Tech Stack:** Rust 2024, Tokio, axum, reqwest, rmcp, serde, schemars, image, tar, zstd, existing Rust integration tests, C++17, existing daemon-cpp test harnesses, cargo test, cargo fmt, cargo clippy, make

---

## File Map

- `Cargo.toml`
  - Add `crates/remote-exec-host` to the workspace members.
- `AGENTS.md`
  - Update the workspace map and project overview to mention `remote-exec-host`.
- `README.md`
  - Update the component/workspace map to mention `remote-exec-host` as the shared host runtime.
- `crates/remote-exec-host/Cargo.toml`
  - New crate manifest for the shared host runtime.
- `crates/remote-exec-host/src/lib.rs`
  - New crate entry point and public re-exports.
- `crates/remote-exec-host/src/config/`
  - New home for host runtime config types, environment capture, and yield-time policy.
- `crates/remote-exec-host/src/host_path.rs`
  - Shared host path resolution and path-policy helpers.
- `crates/remote-exec-host/src/state.rs`
  - Shared runtime state builder and target-info helpers.
- `crates/remote-exec-host/src/exec/`
  - Shared exec runtime moved out of the daemon crate.
- `crates/remote-exec-host/src/patch/`
  - Shared patch runtime moved out of the daemon crate.
- `crates/remote-exec-host/src/image.rs`
  - Shared image runtime moved out of the daemon crate.
- `crates/remote-exec-host/src/transfer/`
  - Shared transfer runtime moved out of the daemon crate.
- `crates/remote-exec-host/src/port_forward.rs`
  - Shared port-forward runtime moved out of the daemon crate.
- `crates/remote-exec-host/src/error.rs`
  - Typed Rust `TransferError` and `ImageError`.
- `crates/remote-exec-daemon/Cargo.toml`
  - Depend on `remote-exec-host`.
- `crates/remote-exec-daemon/src/lib.rs`
  - Re-export the host runtime state and delegate `build_app_state(...)` to the new crate.
- `crates/remote-exec-daemon/src/config/mod.rs`
  - Keep daemon transport config shape stable but add conversions into `remote-exec-host` runtime config.
- `crates/remote-exec-daemon/src/server.rs`
  - Keep HTTP routes and middleware daemon-specific.
- `crates/remote-exec-daemon/src/exec/mod.rs`
  - Convert to thin wrappers over `remote-exec-host::exec`.
- `crates/remote-exec-daemon/src/patch/mod.rs`
  - Convert to thin wrappers over `remote-exec-host::patch`.
- `crates/remote-exec-daemon/src/image.rs`
  - Convert to thin wrappers over `remote-exec-host::image`.
- `crates/remote-exec-daemon/src/transfer/mod.rs`
  - Convert to thin wrappers over `remote-exec-host::transfer` and explicit error mapping.
- `crates/remote-exec-daemon/src/port_forward.rs`
  - Convert to thin wrappers over `remote-exec-host::port_forward`.
- `crates/remote-exec-daemon/tests/*`
  - Keep transport-facing tests and add/adjust assertions for explicit RPC codes where needed.
- `crates/remote-exec-broker/Cargo.toml`
  - Replace the production daemon dependency with `remote-exec-host`; keep daemon as a dev-dependency only for test fixtures.
- `crates/remote-exec-broker/src/lib.rs`
  - Build the local target from `remote-exec-host` and drop daemon production imports.
- `crates/remote-exec-broker/src/config.rs`
  - Build broker-local runtime configs using `remote-exec-host` config types instead of daemon config types.
- `crates/remote-exec-broker/src/local_backend.rs`
  - Replace `LocalDaemonClient` internals with a host-runtime-backed local client.
- `crates/remote-exec-broker/src/local_transfer.rs`
  - Call `remote-exec-host::transfer::archive` directly.
- `crates/remote-exec-broker/src/port_forward.rs`
  - Replace local port state construction with `remote-exec-host` runtime state.
- `crates/remote-exec-broker/src/daemon_client.rs`
  - Keep remote HTTP client behavior but preserve and surface explicit RPC codes.
- `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
  - Prefer RPC codes and status codes over free-form message sniffing.
- `crates/remote-exec-broker/tests/support/spawners.rs`
  - Keep daemon-specific fixture wiring working with `remote-exec-daemon` as a dev-dependency.
- `crates/remote-exec-daemon-cpp/Makefile`
  - Compile any new helper used for explicit internal transfer/image failure categories.
- `crates/remote-exec-daemon-cpp/include/rpc_failures.h`
  - New explicit C++ transfer/image failure categories and helpers.
- `crates/remote-exec-daemon-cpp/src/rpc_failures.cpp`
  - New C++ mapping helpers from internal category to public RPC code string.
- `crates/remote-exec-daemon-cpp/src/server.cpp`
  - Replace message-derived transfer code selection on streaming import/export paths.
- `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
  - Replace `image_error_code(message)` and `transfer_error_code(message)` with explicit failures.
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
  - Add/adjust route-level tests for explicit image/transfer codes.
- `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
  - Add/adjust explicit transfer-code tests.

### Task 1: Scaffold `remote-exec-host` And Split Runtime Config From Daemon Transport

**Files:**
- Modify: `Cargo.toml`
- Modify: `AGENTS.md`
- Modify: `README.md`
- Create: `crates/remote-exec-host/Cargo.toml`
- Create: `crates/remote-exec-host/src/lib.rs`
- Create: `crates/remote-exec-host/src/config/mod.rs`
- Create: `crates/remote-exec-host/src/config/environment.rs`
- Create: `crates/remote-exec-host/src/config/yield_time.rs`
- Create: `crates/remote-exec-host/src/host_path.rs`
- Modify: `crates/remote-exec-daemon/Cargo.toml`
- Modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Modify: `crates/remote-exec-broker/Cargo.toml`
- Modify: `crates/remote-exec-broker/src/config.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --lib`, `cargo test -p remote-exec-broker --lib`

**Testing approach:** `existing tests + targeted verification`
Reason: this is a behavior-preserving extraction seam. Existing daemon and broker config/state tests already prove the current semantics well enough if the refactor is staged carefully.

- [ ] **Step 1: Create the new crate and add it to the workspace**

```toml
# Cargo.toml
[workspace]
members = [
  "crates/remote-exec-proto",
  "crates/remote-exec-host",
  "crates/remote-exec-daemon",
  "crates/remote-exec-broker",
  "crates/remote-exec-pki",
  "crates/remote-exec-admin",
]

# crates/remote-exec-host/Cargo.toml
[package]
name = "remote-exec-host"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
version.workspace = true

[dependencies]
anyhow = { workspace = true }
base64 = { workspace = true }
chardetng = { workspace = true }
encoding_rs = { workspace = true }
futures-util = { workspace = true }
gethostname = { workspace = true }
globset = { workspace = true }
image = { workspace = true }
nix = { workspace = true, optional = true }
os_pipe = { workspace = true }
portable-pty = { workspace = true }
rand = { workspace = true }
remote-exec-proto = { path = "../remote-exec-proto" }
serde = { workspace = true }
serde_json = { workspace = true }
tar = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true }
tokio-util = { workspace = true }
tracing = { workspace = true }
uuid = { workspace = true }
vte = { workspace = true }
zstd = { workspace = true }

[target.'cfg(unix)'.dependencies]
nix = { workspace = true }

[target.'cfg(windows)'.dependencies]
winptyrs = { path = "../../winptyrs/crates/winptyrs", optional = true }

[features]
default = ["winpty"]
winpty = ["dep:winptyrs"]
```

- [ ] **Step 2: Add host-runtime-only config and state types without changing daemon or broker config file shapes**

```rust
// crates/remote-exec-host/src/lib.rs
pub mod config;
pub mod host_path;

pub use config::{
    EmbeddedHostConfig, HostRuntimeConfig, ProcessEnvironment, PtyMode,
    WindowsPtyBackendOverride, YieldTimeConfig, YieldTimeOperation,
    YieldTimeOperationConfig,
};

// crates/remote-exec-host/src/config/mod.rs
#[derive(Debug, Clone)]
pub struct HostRuntimeConfig {
    pub target: String,
    pub default_workdir: PathBuf,
    pub windows_posix_root: Option<PathBuf>,
    pub sandbox: Option<FilesystemSandbox>,
    pub enable_transfer_compression: bool,
    pub allow_login_shell: bool,
    pub pty: PtyMode,
    pub default_shell: Option<String>,
    pub yield_time: YieldTimeConfig,
    pub experimental_apply_patch_target_encoding_autodetect: bool,
    pub process_environment: ProcessEnvironment,
}

#[derive(Debug, Clone)]
pub struct EmbeddedHostConfig {
    pub target: String,
    pub default_workdir: PathBuf,
    pub windows_posix_root: Option<PathBuf>,
    pub sandbox: Option<FilesystemSandbox>,
    pub enable_transfer_compression: bool,
    pub allow_login_shell: bool,
    pub pty: PtyMode,
    pub default_shell: Option<String>,
    pub yield_time: YieldTimeConfig,
    pub experimental_apply_patch_target_encoding_autodetect: bool,
    pub process_environment: ProcessEnvironment,
}

impl EmbeddedHostConfig {
    pub fn into_host_runtime_config(self) -> HostRuntimeConfig {
        HostRuntimeConfig {
            target: self.target,
            default_workdir: self.default_workdir,
            windows_posix_root: self.windows_posix_root,
            sandbox: self.sandbox,
            enable_transfer_compression: self.enable_transfer_compression,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell,
            yield_time: self.yield_time,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: self.process_environment,
        }
    }
}
```

- [ ] **Step 3: Keep the daemon and broker config file formats stable by adding conversion methods instead of reshaping TOML**

```rust
// crates/remote-exec-daemon/src/config/mod.rs
impl DaemonConfig {
    pub fn host_runtime_config(&self) -> remote_exec_host::HostRuntimeConfig {
        remote_exec_host::HostRuntimeConfig {
            target: self.target.clone(),
            default_workdir: self.default_workdir.clone(),
            windows_posix_root: self.windows_posix_root.clone(),
            sandbox: self.sandbox.clone(),
            enable_transfer_compression: self.enable_transfer_compression,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.clone(),
            yield_time: self.yield_time,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: self.process_environment.clone(),
        }
    }
}

// crates/remote-exec-broker/src/config.rs
impl LocalTargetConfig {
    pub fn embedded_host_config(
        &self,
        sandbox: Option<FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> remote_exec_host::EmbeddedHostConfig {
        remote_exec_host::EmbeddedHostConfig {
            target: "local".to_string(),
            default_workdir: self.default_workdir.clone(),
            windows_posix_root: self.windows_posix_root.clone(),
            sandbox,
            enable_transfer_compression,
            allow_login_shell: self.allow_login_shell,
            pty: self.pty,
            default_shell: self.default_shell.clone(),
            yield_time: self.yield_time,
            experimental_apply_patch_target_encoding_autodetect: self
                .experimental_apply_patch_target_encoding_autodetect,
            process_environment: remote_exec_host::ProcessEnvironment::capture_current(),
        }
    }
}
```

- [ ] **Step 4: Update the repo docs for the extra internal crate and keep the daemon using its current runtime until Task 2**

```md
<!-- README.md -->
- `remote-exec-host`
  - Shared host runtime used by the Rust daemon and broker-host `local` execution path.

<!-- AGENTS.md -->
- `crates/remote-exec-host/src/`: shared host-runtime config, state, exec, patch, image, transfer, and port-forward logic used by the Rust daemon and broker-local path.
```

- [ ] **Step 5: Run the focused daemon/broker library suites**

Run:

```bash
cargo test -p remote-exec-daemon --lib
cargo test -p remote-exec-broker --lib
```

Expected: PASS, proving the config/state extraction did not change current daemon or broker config semantics.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml AGENTS.md README.md \
  crates/remote-exec-host \
  crates/remote-exec-daemon/Cargo.toml crates/remote-exec-daemon/src/config/mod.rs crates/remote-exec-daemon/src/lib.rs \
  crates/remote-exec-broker/Cargo.toml crates/remote-exec-broker/src/config.rs
git commit -m "refactor: scaffold shared host runtime crate"
```

### Task 2: Move Rust Host Runtime Modules Into `remote-exec-host` And Leave Daemon Transport Wrappers

**Files:**
- Create: `crates/remote-exec-host/src/state.rs`
- Create/Move: `crates/remote-exec-host/src/exec/*`
- Create/Move: `crates/remote-exec-host/src/patch/*`
- Create/Move: `crates/remote-exec-host/src/image.rs`
- Create/Move: `crates/remote-exec-host/src/transfer/*`
- Create/Move: `crates/remote-exec-host/src/port_forward.rs`
- Modify: `crates/remote-exec-host/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-daemon/src/patch/mod.rs`
- Modify: `crates/remote-exec-daemon/src/image.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/src/port_forward.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture`, `cargo test -p remote-exec-daemon --test patch_rpc`, `cargo test -p remote-exec-daemon --test image_rpc`, `cargo test -p remote-exec-daemon --test transfer_rpc`, `cargo test -p remote-exec-daemon --test port_forward_rpc`

**Testing approach:** `characterization/integration test`
Reason: the daemon already has high-value RPC integration tests across exec, patch, image, transfer, and port forwarding. Keep behavior constant while moving implementation ownership.

- [ ] **Step 1: Move the runtime modules into the new crate with `git mv` so history stays readable**

```bash
mkdir -p crates/remote-exec-host/src/exec/session crates/remote-exec-host/src/exec/shell
mkdir -p crates/remote-exec-host/src/patch
mkdir -p crates/remote-exec-host/src/transfer/archive

git mv crates/remote-exec-daemon/src/exec/handlers.rs crates/remote-exec-host/src/exec/handlers.rs
git mv crates/remote-exec-daemon/src/exec/locale.rs crates/remote-exec-host/src/exec/locale.rs
git mv crates/remote-exec-daemon/src/exec/output.rs crates/remote-exec-host/src/exec/output.rs
git mv crates/remote-exec-daemon/src/exec/session crates/remote-exec-host/src/exec/session
git mv crates/remote-exec-daemon/src/exec/shell crates/remote-exec-host/src/exec/shell
git mv crates/remote-exec-daemon/src/exec/store.rs crates/remote-exec-host/src/exec/store.rs
git mv crates/remote-exec-daemon/src/exec/support.rs crates/remote-exec-host/src/exec/support.rs
git mv crates/remote-exec-daemon/src/exec/transcript.rs crates/remote-exec-host/src/exec/transcript.rs
git mv crates/remote-exec-daemon/src/patch/* crates/remote-exec-host/src/patch/
git mv crates/remote-exec-daemon/src/image.rs crates/remote-exec-host/src/image.rs
git mv crates/remote-exec-daemon/src/transfer/mod.rs crates/remote-exec-host/src/transfer/mod.rs
git mv crates/remote-exec-daemon/src/transfer/archive crates/remote-exec-host/src/transfer/archive
git mv crates/remote-exec-daemon/src/port_forward.rs crates/remote-exec-host/src/port_forward.rs
```

- [ ] **Step 2: Add the shared runtime state and repoint moved modules at `remote-exec-host` names**

```rust
// crates/remote-exec-host/src/lib.rs
pub mod exec;
pub mod state;
pub mod image;
pub mod patch;
pub mod port_forward;
pub mod transfer;

pub use state::{HostRuntimeState, build_runtime_state, target_info_response};

// crates/remote-exec-host/src/state.rs
#[derive(Clone)]
pub struct HostRuntimeState {
    pub config: Arc<HostRuntimeConfig>,
    pub default_shell: String,
    pub sandbox: Option<CompiledFilesystemSandbox>,
    pub supports_pty: bool,
    pub supports_transfer_compression: bool,
    pub windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    pub daemon_instance_id: String,
    pub sessions: crate::exec::store::SessionStore,
    pub port_forwards: crate::port_forward::PortForwardState,
}

pub fn build_runtime_state(mut config: HostRuntimeConfig) -> anyhow::Result<HostRuntimeState> {
    config.normalize_paths();
    config.validate()?;
    let sandbox = config
        .sandbox
        .as_ref()
        .map(|sandbox| compile_filesystem_sandbox(crate::host_path::host_path_policy(), sandbox))
        .transpose()?;
    let default_shell = crate::exec::shell::resolve_default_shell(
        config.default_shell.as_deref(),
        &config.process_environment,
        config.windows_posix_root.as_deref(),
    )?;
    crate::exec::session::validate_pty_mode(config.pty)?;
    let supports_pty = crate::exec::session::supports_pty_for_mode(config.pty);
    let windows_pty_backend_override =
        crate::exec::session::windows_pty_backend_override_for_mode(config.pty)?;

    Ok(HostRuntimeState {
        config: Arc::new(config),
        default_shell,
        sandbox,
        supports_pty,
        supports_transfer_compression: config.enable_transfer_compression,
        windows_pty_backend_override,
        daemon_instance_id: uuid::Uuid::new_v4().to_string(),
        sessions: crate::exec::store::SessionStore::new(64),
        port_forwards: crate::port_forward::PortForwardState::default(),
    })
}

// crates/remote-exec-host/src/exec/mod.rs
mod handlers;
mod locale;
mod output;
pub mod session;
pub(crate) mod shell;
pub mod store;
mod support;
pub mod transcript;
#[cfg(all(windows, feature = "winpty"))]
mod winpty;

pub use handlers::{exec_start_local, exec_write_local};
pub use support::{
    ensure_sandbox_access, internal_error, resolve_input_path,
    resolve_input_path_with_windows_posix_root, resolve_workdir, rpc_error,
};
```

- [ ] **Step 3: Re-export the new state from the daemon crate and convert daemon modules into thin transport wrappers**

```rust
// crates/remote-exec-daemon/src/lib.rs
pub type AppState = remote_exec_host::HostRuntimeState;

pub fn build_app_state(config: config::DaemonConfig) -> Result<AppState> {
    remote_exec_host::build_runtime_state(config.host_runtime_config())
}

pub fn target_info_response(state: &AppState) -> TargetInfoResponse {
    remote_exec_host::target_info_response(state, env!("CARGO_PKG_VERSION"))
}

// crates/remote-exec-daemon/src/image.rs
pub async fn read_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::image::read_image_local(state, req).await.map(Json)
}

// crates/remote-exec-daemon/src/patch/mod.rs
pub async fn apply_patch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::patch::apply_patch_local(state, req).await.map(Json)
}

// crates/remote-exec-daemon/src/exec/mod.rs
pub use remote_exec_host::exec::{
    ensure_sandbox_access, internal_error, resolve_input_path,
    resolve_input_path_with_windows_posix_root, resolve_workdir, rpc_error,
};

pub async fn exec_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::exec::exec_start_local(state, req).await.map(Json)
}

pub async fn exec_write(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)> {
    remote_exec_host::exec::exec_write_local(state, req).await.map(Json)
}
```

- [ ] **Step 4: Run the focused daemon RPC suites**

Run:

```bash
cargo test -p remote-exec-daemon --test exec_rpc -- --nocapture
cargo test -p remote-exec-daemon --test patch_rpc
cargo test -p remote-exec-daemon --test image_rpc
cargo test -p remote-exec-daemon --test transfer_rpc
cargo test -p remote-exec-daemon --test port_forward_rpc
```

Expected: PASS, proving the daemon still exposes the same host behavior after the implementation ownership move.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-host/src \
  crates/remote-exec-daemon/src/lib.rs \
  crates/remote-exec-daemon/src/exec/mod.rs \
  crates/remote-exec-daemon/src/patch/mod.rs \
  crates/remote-exec-daemon/src/image.rs \
  crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/port_forward.rs
git commit -m "refactor: move rust host runtime out of daemon crate"
```

### Task 3: Rewire Broker Local Execution, Transfer, And Port Forwarding To `remote-exec-host`

**Files:**
- Modify: `crates/remote-exec-broker/Cargo.toml`
- Modify: `crates/remote-exec-broker/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Modify: `crates/remote-exec-broker/src/port_forward.rs`
- Modify: `crates/remote-exec-broker/src/client.rs`
- Modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`, `cargo test -p remote-exec-broker --test mcp_assets`, `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** `existing tests + targeted verification`
Reason: the broker MCP surface already has strong coverage for `local`, mixed-target, and transfer semantics. Use those tests as the contract while removing the production broker -> daemon dependency.

- [ ] **Step 1: Replace the broker’s production daemon dependency with the host runtime and keep daemon only in dev-dependencies**

```toml
# crates/remote-exec-broker/Cargo.toml
[features]
default = ["broker-tls"]
broker-tls = [
    "dep:rustls",
    "dep:rustls-pemfile",
    "reqwest/rustls-no-provider",
    "rmcp/reqwest-tls-no-provider",
]

[dependencies]
remote-exec-host = { path = "../remote-exec-host" }
remote-exec-proto = { path = "../remote-exec-proto" }

[dev-dependencies]
remote-exec-daemon = { path = "../remote-exec-daemon" }
remote-exec-pki = { path = "../remote-exec-pki" }
```

- [ ] **Step 2: Replace `LocalDaemonClient` internals with a `remote-exec-host` runtime**

```rust
// crates/remote-exec-broker/src/local_backend.rs
#[derive(Clone)]
pub struct LocalDaemonClient {
    state: Arc<remote_exec_host::HostRuntimeState>,
}

impl LocalDaemonClient {
    pub fn new(
        config: &crate::config::LocalTargetConfig,
        sandbox: Option<remote_exec_proto::sandbox::FilesystemSandbox>,
        enable_transfer_compression: bool,
    ) -> anyhow::Result<Self> {
        let embedded = config.embedded_host_config(sandbox, enable_transfer_compression);
        let state =
            remote_exec_host::build_runtime_state(embedded.into_host_runtime_config())?;
        Ok(Self {
            state: Arc::new(state),
        })
    }

    pub async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        Ok(remote_exec_host::target_info_response(
            &self.state,
            env!("CARGO_PKG_VERSION"),
        ))
    }
}
```

- [ ] **Step 3: Point broker-local transfer and port-forward helpers at `remote-exec-host` instead of daemon modules**

```rust
// crates/remote-exec-broker/src/local_transfer.rs
let exported = remote_exec_host::transfer::archive::export_path_to_file(
    path,
    archive_path,
    request.compression.clone(),
    request.symlink_mode.clone(),
    &request.exclude,
    sandbox,
    None,
)
.await?;

// crates/remote-exec-broker/src/port_forward.rs
#[derive(Clone)]
pub struct LocalPortClient {
    state: Arc<remote_exec_host::HostRuntimeState>,
}

impl LocalPortClient {
    fn global() -> Self {
        static STATE: OnceLock<Arc<remote_exec_host::HostRuntimeState>> = OnceLock::new();
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
                Arc::new(
                    remote_exec_host::build_runtime_state(config.into_host_runtime_config())
                        .expect("construct local port runtime"),
                )
            })
            .clone();
        Self { state }
    }
}
```

- [ ] **Step 4: Remove the broker’s daemon-side crypto install call and rely only on broker TLS setup**

```rust
// crates/remote-exec-broker/src/lib.rs
pub fn install_crypto_provider() {
    broker_tls::install_crypto_provider();
}
```

- [ ] **Step 5: Run the focused broker MCP suites**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_exec -- --nocapture
cargo test -p remote-exec-broker --test mcp_assets
cargo test -p remote-exec-broker --test mcp_transfer
cargo test -p remote-exec-broker --test multi_target -- --nocapture
```

Expected: PASS, proving the broker `local` path and mixed target routing still behave the same after the dependency cleanup.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-broker/Cargo.toml \
  crates/remote-exec-broker/src/lib.rs \
  crates/remote-exec-broker/src/config.rs \
  crates/remote-exec-broker/src/local_backend.rs \
  crates/remote-exec-broker/src/local_transfer.rs \
  crates/remote-exec-broker/src/port_forward.rs \
  crates/remote-exec-broker/src/client.rs \
  crates/remote-exec-broker/tests/support/spawners.rs \
  crates/remote-exec-broker/tests/support/stub_daemon.rs
git commit -m "refactor: remove broker dependency on daemon internals"
```

### Task 4: Replace Rust Message-Sniffing With Typed `TransferError` And `ImageError`

**Files:**
- Create: `crates/remote-exec-host/src/error.rs`
- Modify: `crates/remote-exec-host/src/lib.rs`
- Modify: `crates/remote-exec-host/src/image.rs`
- Modify: `crates/remote-exec-host/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/src/image.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-daemon/tests/image_rpc.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test image_rpc`, `cargo test -p remote-exec-daemon --test transfer_rpc`, `cargo test -p remote-exec-broker --test mcp_transfer`

**Testing approach:** `existing tests + targeted verification`
Reason: the public transfer/image codes already exist today, so the valuable check here is preserving those transport-facing assertions while moving the classifier from message text to typed internal categories.

- [ ] **Step 1: Add explicit transport-level assertions that capture the current public codes before changing the internals**

```rust
// crates/remote-exec-daemon/tests/image_rpc.rs
assert_eq!(err.code, "image_missing");
assert!(err.message.contains("unable to locate image at"));

assert_eq!(err.code, "invalid_detail");
assert!(err.message.contains("only supports `original`"));

// crates/remote-exec-daemon/tests/transfer_rpc.rs
assert_eq!(err.code, "transfer_path_not_absolute");
assert_eq!(err.code, "transfer_destination_exists");
assert_eq!(err.code, "transfer_destination_unsupported");
assert_eq!(err.code, "transfer_source_missing");
```

- [ ] **Step 2: Run the focused Rust transport tests to capture the current transport contract before the internal refactor**

Run:

```bash
cargo test -p remote-exec-daemon --test image_rpc
cargo test -p remote-exec-daemon --test transfer_rpc
```

Expected: PASS, capturing the current outward-facing image and transfer code contract before the implementation changes underneath.

- [ ] **Step 3: Introduce typed host-runtime errors and map them directly in the daemon transport layer**

```rust
// crates/remote-exec-host/src/lib.rs
pub mod error;

// crates/remote-exec-host/src/error.rs
#[derive(Debug, Clone)]
pub enum TransferError {
    SandboxDenied(String),
    PathNotAbsolute(String),
    DestinationExists(String),
    ParentMissing(String),
    DestinationUnsupported(String),
    CompressionUnsupported(String),
    SourceUnsupported(String),
    SourceMissing(String),
    Internal(String),
}

impl TransferError {
    pub fn rpc_code(&self) -> &'static str {
        match self {
            Self::SandboxDenied(_) => "sandbox_denied",
            Self::PathNotAbsolute(_) => "transfer_path_not_absolute",
            Self::DestinationExists(_) => "transfer_destination_exists",
            Self::ParentMissing(_) => "transfer_parent_missing",
            Self::DestinationUnsupported(_) => "transfer_destination_unsupported",
            Self::CompressionUnsupported(_) => "transfer_compression_unsupported",
            Self::SourceUnsupported(_) => "transfer_source_unsupported",
            Self::SourceMissing(_) => "transfer_source_missing",
            Self::Internal(_) => "transfer_failed",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::SandboxDenied(message)
            | Self::PathNotAbsolute(message)
            | Self::DestinationExists(message)
            | Self::ParentMissing(message)
            | Self::DestinationUnsupported(message)
            | Self::CompressionUnsupported(message)
            | Self::SourceUnsupported(message)
            | Self::SourceMissing(message)
            | Self::Internal(message) => message,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ImageError {
    SandboxDenied(String),
    InvalidDetail(String),
    Missing(String),
    NotFile(String),
    DecodeFailed(String),
    Internal(String),
}

impl ImageError {
    pub fn rpc_code(&self) -> &'static str {
        match self {
            Self::SandboxDenied(_) => "sandbox_denied",
            Self::InvalidDetail(_) => "invalid_detail",
            Self::Missing(_) => "image_missing",
            Self::NotFile(_) => "image_not_file",
            Self::DecodeFailed(_) | Self::Internal(_) => "image_decode_failed",
        }
    }
}

// crates/remote-exec-daemon/src/transfer/mod.rs
pub fn map_transfer_error(err: remote_exec_host::error::TransferError) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error(err.rpc_code(), err.message().to_string())
}

// crates/remote-exec-daemon/src/image.rs
fn map_image_error(err: remote_exec_host::error::ImageError) -> (StatusCode, Json<RpcErrorBody>) {
    crate::exec::rpc_error(err.rpc_code(), err.message().to_string())
}
```

- [ ] **Step 4: Update broker-side path-info fallback logic to use RPC codes first and status fallback second**

```rust
// crates/remote-exec-broker/src/tools/transfer/endpoints.rs
fn path_info_missing_or_unsupported(err: &crate::daemon_client::DaemonClientError) -> bool {
    match err {
        crate::daemon_client::DaemonClientError::Rpc { code, status, .. } => {
            matches!(
                code.as_deref(),
                Some("unsupported_operation") | Some("transfer_destination_unsupported")
            ) || *status == reqwest::StatusCode::NOT_FOUND
                || *status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        }
        _ => false,
    }
}
```

- [ ] **Step 5: Run the focused Rust image/transfer suites and broker transfer suite**

Run:

```bash
cargo test -p remote-exec-daemon --test image_rpc
cargo test -p remote-exec-daemon --test transfer_rpc
cargo test -p remote-exec-broker --test mcp_transfer
```

Expected: PASS, proving the Rust path now chooses stable public codes without sniffing free-form messages.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-host/src/error.rs \
  crates/remote-exec-host/src/lib.rs \
  crates/remote-exec-host/src/image.rs \
  crates/remote-exec-host/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/image.rs \
  crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-broker/src/tools/transfer/endpoints.rs \
  crates/remote-exec-broker/src/daemon_client.rs \
  crates/remote-exec-daemon/tests/image_rpc.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "refactor: type rust transfer and image errors"
```

### Task 5: Give The C++ Daemon Explicit Internal Transfer And Image Failure Categories

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Create: `crates/remote-exec-daemon-cpp/include/rpc_failures.h`
- Create: `crates/remote-exec-daemon-cpp/src/rpc_failures.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`, `make -C crates/remote-exec-daemon-cpp test-host-transfer`, `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** `existing tests + targeted verification`
Reason: the C++ daemon already exposes the current public codes. Lock those route-level assertions in first, then switch the implementation from message-derived codes to explicit internal categories without changing the outward contract.

- [ ] **Step 1: Add C++ route and transfer assertions that lock in the current public codes before changing the internals**

```cpp
// crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
assert_json_error_code(response.body, "image_missing");
assert_json_error_code(response.body, "invalid_detail");
assert_json_error_code(response.body, "transfer_path_not_absolute");
assert_json_error_code(response.body, "transfer_destination_unsupported");

// crates/remote-exec-daemon-cpp/tests/test_transfer.cpp
assert_equal(transfer_error_code_name(TransferRpcCode::SourceMissing), "transfer_source_missing");
assert_equal(transfer_error_code_name(TransferRpcCode::CompressionUnsupported), "transfer_compression_unsupported");
```

- [ ] **Step 2: Run the focused C++ tests to capture the current public contract before the classifier rewrite**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-server-routes
make -C crates/remote-exec-daemon-cpp test-host-transfer
```

Expected: PASS, confirming the route-level and transfer-level public codes before the internal rewrite.

- [ ] **Step 3: Add explicit failure types and use them in both C++ server entry points**

```cpp
// crates/remote-exec-daemon-cpp/include/rpc_failures.h
enum class TransferRpcCode {
    SandboxDenied,
    PathNotAbsolute,
    DestinationExists,
    ParentMissing,
    DestinationUnsupported,
    CompressionUnsupported,
    SourceUnsupported,
    SourceMissing,
    TransferFailed,
};

enum class ImageRpcCode {
    SandboxDenied,
    InvalidDetail,
    Missing,
    NotFile,
    DecodeFailed,
};

struct TransferFailure {
    TransferRpcCode code;
    std::string message;
};

struct ImageFailure {
    ImageRpcCode code;
    std::string message;
};

const char* transfer_error_code_name(TransferRpcCode code);
const char* image_error_code_name(ImageRpcCode code);

// crates/remote-exec-daemon-cpp/src/server_routes.cpp
catch (const ImageFailure& failure) {
    log_message(LOG_WARN, "server", "image/read failed: " + failure.message);
    write_rpc_error(response, 400, image_error_code_name(failure.code), failure.message);
}

catch (const TransferFailure& failure) {
    log_message(LOG_WARN, "server", "transfer/export failed: " + failure.message);
    write_rpc_error(response, 400, transfer_error_code_name(failure.code), failure.message);
}
```

- [ ] **Step 4: Route existing helper failures through explicit categories instead of `what()` sniffing**

```cpp
// crates/remote-exec-daemon-cpp/src/server_routes.cpp
ImageFailure make_invalid_detail(const std::string& detail) {
    return ImageFailure{
        ImageRpcCode::InvalidDetail,
        "view_image.detail only supports `original`; omit `detail` for default original behavior, got `" + detail + "`"
    };
}

TransferFailure make_transfer_path_not_absolute() {
    return TransferFailure{
        TransferRpcCode::PathNotAbsolute,
        "transfer path is not absolute"
    };
}
```

- [ ] **Step 5: Run the focused C++ suites plus the host POSIX check**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-server-routes
make -C crates/remote-exec-daemon-cpp test-host-transfer
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: PASS, proving the C++ daemon now chooses transfer/image public codes explicitly and still passes the existing host-native suite.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-daemon-cpp/Makefile \
  crates/remote-exec-daemon-cpp/include/rpc_failures.h \
  crates/remote-exec-daemon-cpp/src/rpc_failures.cpp \
  crates/remote-exec-daemon-cpp/src/server.cpp \
  crates/remote-exec-daemon-cpp/src/server_routes.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
  crates/remote-exec-daemon-cpp/tests/test_transfer.cpp
git commit -m "refactor: give cpp daemon explicit rpc failure categories"
```

### Task 6: Run The Full Quality Gate And Check The Final Dependency Topology

**Files:**
- Test/Verify only

**Testing approach:** `no new tests needed`
Reason: this task is a final integration gate over already-verified slices.

- [ ] **Step 1: Confirm the broker no longer depends on the daemon crate in production**

Run:

```bash
rg -n "remote-exec-host|remote-exec-daemon" crates/remote-exec-broker/Cargo.toml
```

Expected: `remote-exec-host` appears under `[dependencies]` and `remote-exec-daemon` appears only under `[dev-dependencies]`.

- [ ] **Step 2: Run the Rust workspace verification gate**

Run:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: PASS.

- [ ] **Step 3: Run the C++ focused verification gate**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-transfer
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: verify host runtime boundary cleanup"
```
