# Remote Exec MCP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a standalone Rust workspace with a remote-first MCP broker and per-machine Linux daemon that implement `exec_command`, `write_stdin`, `apply_patch`, and `view_image` across multiple target machines.

**Architecture:** The broker exposes the public MCP tool surface over stdio using `rmcp`, validates `target`, allocates opaque public `session_id` values, and forwards requests to the correct daemon over mTLS JSON/HTTP. Each daemon owns target-local execution semantics: PTY and pipe process management, live-session polling, patch parsing and application, and image loading and resizing.

**Tech Stack:** Rust 2024, tokio, rmcp 0.15.x, axum 0.8, reqwest 0.12, rustls 0.23, rcgen, portable-pty 0.9.0, serde, schemars, image, tracing

---

## Planned File Layout

- `Cargo.toml`
  - Workspace manifest for broker, daemon, and shared protocol crates.
- `.gitignore`
  - Ignore `target/`, editor state, and generated TLS fixtures.
- `crates/remote-exec-proto/Cargo.toml`
  - Shared dependency set for protocol and public tool argument/result types.
- `crates/remote-exec-proto/src/lib.rs`
  - Re-export shared modules.
- `crates/remote-exec-proto/src/rpc.rs`
  - Internal broker-daemon request/response structs and error codes.
- `crates/remote-exec-proto/src/public.rs`
  - Public MCP tool input and structured result types.
- `crates/remote-exec-daemon/Cargo.toml`
  - Daemon package manifest and daemon-specific dependencies.
- `crates/remote-exec-daemon/src/lib.rs`
  - `run`, router construction, and daemon application state.
- `crates/remote-exec-daemon/src/main.rs`
  - CLI entrypoint that loads config and launches the daemon.
- `crates/remote-exec-daemon/src/config.rs`
  - TOML config loading for target name, listen address, default cwd, and TLS files.
- `crates/remote-exec-daemon/src/tls.rs`
  - rustls server configuration with required client certificate verification.
- `crates/remote-exec-daemon/src/server.rs`
  - Axum route registration and JSON error translation.
- `crates/remote-exec-daemon/src/exec/mod.rs`
  - `ExecStart` and `ExecWrite` handlers.
- `crates/remote-exec-daemon/src/exec/session.rs`
  - Live session store, PTY/pipes spawn logic, and session lifecycle.
- `crates/remote-exec-daemon/src/exec/transcript.rs`
  - Head-tail transcript retention, chunk IDs, and token-count metadata.
- `crates/remote-exec-daemon/src/patch/mod.rs`
  - Patch verification and application orchestration.
- `crates/remote-exec-daemon/src/patch/parser.rs`
  - Standalone patch parser and hunk matcher.
- `crates/remote-exec-daemon/src/image.rs`
  - `view_image` file validation, resize-to-fit, and data URL generation.
- `crates/remote-exec-daemon/tests/health.rs`
  - mTLS and `TargetInfo` integration tests.
- `crates/remote-exec-daemon/tests/exec_rpc.rs`
  - Live session, polling, TTY, and exit cleanup tests.
- `crates/remote-exec-daemon/tests/patch_rpc.rs`
  - Patch grammar, overwrite semantics, and non-atomic failure tests.
- `crates/remote-exec-daemon/tests/image_rpc.rs`
  - Resize/default/original detail behavior tests.
- `crates/remote-exec-broker/Cargo.toml`
  - Broker package manifest and MCP dependencies.
- `crates/remote-exec-broker/src/lib.rs`
  - Broker bootstrap and MCP server assembly.
- `crates/remote-exec-broker/src/main.rs`
  - CLI entrypoint that loads config and serves MCP over stdio.
- `crates/remote-exec-broker/src/config.rs`
  - TOML config loading for target endpoints and trust settings.
- `crates/remote-exec-broker/src/session_store.rs`
  - Public `session_id` allocation and `(target, daemon_session_id)` mapping.
- `crates/remote-exec-broker/src/daemon_client.rs`
  - mTLS HTTP client for all daemon RPCs.
- `crates/remote-exec-broker/src/mcp_server.rs`
  - `rmcp` tool registration and common `CallToolResult` helpers.
- `crates/remote-exec-broker/src/tools/mod.rs`
  - Tool module exports.
- `crates/remote-exec-broker/src/tools/exec.rs`
  - `exec_command` and `write_stdin` broker-side implementation.
- `crates/remote-exec-broker/src/tools/patch.rs`
  - `apply_patch` broker-side implementation.
- `crates/remote-exec-broker/src/tools/image.rs`
  - `view_image` broker-side implementation.
- `crates/remote-exec-broker/tests/mcp_exec.rs`
  - Broker tool contract tests for command execution and session routing.
- `crates/remote-exec-broker/tests/mcp_assets.rs`
  - Broker tool contract tests for patch and image tools.
- `tests/e2e/multi_target.rs`
  - Whole-system tests with two daemons and one broker.
- `configs/broker.example.toml`
  - Example broker config with multiple targets.
- `configs/daemon.example.toml`
  - Example daemon config for a single target.
- `README.md`
  - Project runbook, trust model, and local development commands.

### Task 1: Bootstrap The Rust Workspace And Shared Types

**Files:**
- Create: `Cargo.toml`
- Create: `.gitignore`
- Create: `crates/remote-exec-proto/Cargo.toml`
- Create: `crates/remote-exec-proto/src/lib.rs`
- Create: `crates/remote-exec-proto/src/rpc.rs`
- Create: `crates/remote-exec-proto/src/public.rs`
- Create: `crates/remote-exec-daemon/Cargo.toml`
- Create: `crates/remote-exec-broker/Cargo.toml`
- Test/Verify: `cargo test -p remote-exec-proto`

**Testing approach:** `existing tests + targeted verification`
Reason: this task is mostly workspace scaffolding and shared type definitions, so a focused compile/test check is enough.

- [ ] **Step 1: Create the workspace skeleton**

```bash
mkdir -p crates configs docs/superpowers/plans tests/e2e
cargo new --lib crates/remote-exec-proto --vcs none
cargo new --bin crates/remote-exec-daemon --vcs none
cargo new --bin crates/remote-exec-broker --vcs none
```

- [ ] **Step 2: Run the structural verification**

Run: `find crates -maxdepth 2 -name Cargo.toml | sort`
Expected: output includes exactly `crates/remote-exec-broker/Cargo.toml`, `crates/remote-exec-daemon/Cargo.toml`, and `crates/remote-exec-proto/Cargo.toml`

- [ ] **Step 3: Implement the workspace manifest and shared protocol types**

```toml
# Cargo.toml
[workspace]
members = [
  "crates/remote-exec-proto",
  "crates/remote-exec-daemon",
  "crates/remote-exec-broker",
]
resolver = "2"

[workspace.package]
edition = "2024"
license = "Apache-2.0"
version = "0.1.0"

[workspace.dependencies]
anyhow = "1"
axum = { version = "0.8", default-features = false, features = ["http1", "http2", "json", "tokio"] }
base64 = "0.22"
bytes = "1"
clap = { version = "4.5", features = ["derive"] }
http = "1"
hyper = { version = "1", features = ["server", "http1", "http2"] }
hyper-util = { version = "0.1", features = ["server", "http1", "http2", "tokio"] }
image = { version = "0.25", default-features = false, features = ["gif", "jpeg", "png", "webp"] }
portable-pty = "0.9.0"
rand = "0.8"
rcgen = "0.13"
reqwest = { version = "0.12", default-features = false, features = ["json", "http2", "rustls-tls"] }
rmcp = { version = "0.15.0", default-features = false, features = ["server"] }
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12"] }
rustls-pemfile = "2"
schemars = "0.8"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tempfile = "3"
tokio = { version = "1", features = ["fs", "io-util", "macros", "process", "rt-multi-thread", "signal", "sync", "time"] }
tokio-rustls = "0.26"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
uuid = { version = "1", features = ["serde", "v4"] }
```

```gitignore
# .gitignore
/target
/.idea
/.vscode
*.pem
*.key
*.crt
```

```toml
# crates/remote-exec-proto/Cargo.toml
[package]
name = "remote-exec-proto"
edition.workspace = true
license.workspace = true
version.workspace = true

[dependencies]
schemars = { workspace = true }
serde = { workspace = true }
```

```rust
// crates/remote-exec-proto/src/lib.rs
pub mod public;
pub mod rpc;
```

```rust
// crates/remote-exec-proto/src/rpc.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthCheckResponse {
    pub status: String,
    pub daemon_version: String,
    pub daemon_instance_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TargetInfoResponse {
    pub target: String,
    pub daemon_version: String,
    pub daemon_instance_id: String,
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    pub supports_pty: bool,
    pub supports_image_read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecStartRequest {
    pub cmd: String,
    pub workdir: Option<String>,
    pub shell: Option<String>,
    pub tty: bool,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
    pub login: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWriteRequest {
    pub daemon_session_id: String,
    pub chars: String,
    pub yield_time_ms: Option<u64>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecResponse {
    pub daemon_session_id: Option<String>,
    pub running: bool,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub original_token_count: Option<u32>,
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyRequest {
    pub patch: String,
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchApplyResponse {
    pub output: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadRequest {
    pub path: String,
    pub workdir: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageReadResponse {
    pub image_url: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorBody {
    pub code: &'static str,
    pub message: String,
}
```

```rust
// crates/remote-exec-proto/src/public.rs
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecCommandInput {
    pub target: String,
    pub cmd: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub tty: bool,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub login: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteStdinInput {
    pub session_id: String,
    #[serde(default)]
    pub chars: Option<String>,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandToolResult {
    pub target: String,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub session_id: Option<String>,
    pub original_token_count: Option<u32>,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyPatchInput {
    pub target: String,
    pub input: String,
    #[serde(default)]
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ApplyPatchResult {
    pub target: String,
    pub output: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewImageInput {
    pub target: String,
    pub path: String,
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ViewImageResult {
    pub target: String,
    pub image_url: String,
    pub detail: Option<String>,
}
```

- [ ] **Step 4: Run the shared-type verification**

Run: `cargo test -p remote-exec-proto`
Expected: PASS with `0 failed`

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml .gitignore crates/remote-exec-proto crates/remote-exec-daemon/Cargo.toml crates/remote-exec-broker/Cargo.toml
git commit -m "chore: scaffold remote exec workspace"
```

### Task 2: Build The Daemon Skeleton With mTLS, Health, And Target Info

**Files:**
- Create: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/main.rs`
- Create: `crates/remote-exec-daemon/src/config.rs`
- Create: `crates/remote-exec-daemon/src/tls.rs`
- Create: `crates/remote-exec-daemon/src/server.rs`
- Create: `crates/remote-exec-daemon/tests/health.rs`
- Test/Verify: `cargo test -p remote-exec-daemon health -- --nocapture`

**Testing approach:** `characterization/integration test`
Reason: this task crosses config loading, TLS, and HTTP routing, so an integration test gives the cleanest failure signal.

- [ ] **Step 1: Write the failing daemon health test**

```rust
// crates/remote-exec-daemon/tests/health.rs
use remote_exec_proto::rpc::TargetInfoResponse;
use reqwest::StatusCode;

#[tokio::test]
async fn target_info_is_available_over_mutual_tls() {
    let fixture = test_support::spawn_daemon("builder-a").await;

    let health = fixture
        .client
        .post(fixture.url("/v1/health"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let info = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json::<TargetInfoResponse>()
        .await
        .unwrap();

    assert_eq!(info.target, "builder-a");
    assert_eq!(info.platform, "linux");
    assert!(info.supports_pty);
    assert!(info.supports_image_read);
}

mod test_support {
    use std::net::SocketAddr;
    use tempfile::TempDir;

    pub struct DaemonFixture {
        pub _tempdir: TempDir,
        pub client: reqwest::Client,
        pub addr: SocketAddr,
    }

    impl DaemonFixture {
        pub fn url(&self, path: &str) -> String {
            format!("https://{}{}", self.addr, path)
        }
    }

    pub async fn spawn_daemon(target: &str) -> DaemonFixture {
        let tempdir = tempfile::tempdir().unwrap();
        let certs = write_test_certs(tempdir.path());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let workdir = tempdir.path().join("workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let config = remote_exec_daemon::config::DaemonConfig {
            target: target.to_string(),
            listen: addr,
            default_workdir: workdir,
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: certs.daemon_cert.clone(),
                key_pem: certs.daemon_key.clone(),
                ca_pem: certs.ca_cert.clone(),
            },
        };

        tokio::spawn(remote_exec_daemon::run(config));

        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(reqwest::Certificate::from_pem(&std::fs::read(&certs.ca_cert).unwrap()).unwrap())
            .identity(reqwest::Identity::from_pem(
                &[
                    std::fs::read(&certs.client_cert).unwrap(),
                    std::fs::read(&certs.client_key).unwrap(),
                ]
                .concat(),
            ).unwrap())
            .build()
            .unwrap();

        wait_until_ready(&client, addr).await;
        DaemonFixture { _tempdir: tempdir, client, addr }
    }

    struct TestCerts {
        ca_cert: std::path::PathBuf,
        client_cert: std::path::PathBuf,
        client_key: std::path::PathBuf,
        daemon_cert: std::path::PathBuf,
        daemon_key: std::path::PathBuf,
    }

    fn write_test_certs(dir: &std::path::Path) -> TestCerts {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca_cert = rcgen::CertificateParams::new(vec![])
            .unwrap()
            .self_signed(&ca_key)
            .unwrap();

        let mut daemon_params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        daemon_params.subject_alt_names.push(rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap()));
        let daemon_key = rcgen::KeyPair::generate().unwrap();
        let daemon_cert = daemon_params.signed_by(&daemon_key, &ca_cert, &ca_key).unwrap();

        let client_key = rcgen::KeyPair::generate().unwrap();
        let client_cert = rcgen::CertificateParams::new(vec!["broker".to_string()])
            .unwrap()
            .signed_by(&client_key, &ca_cert, &ca_key)
            .unwrap();

        let ca_cert_path = dir.join("ca.pem");
        let daemon_cert_path = dir.join("daemon.pem");
        let daemon_key_path = dir.join("daemon.key");
        let client_cert_path = dir.join("client.pem");
        let client_key_path = dir.join("client.key");

        std::fs::write(&ca_cert_path, ca_cert.pem()).unwrap();
        std::fs::write(&daemon_cert_path, daemon_cert.pem()).unwrap();
        std::fs::write(&daemon_key_path, daemon_key.serialize_pem()).unwrap();
        std::fs::write(&client_cert_path, client_cert.pem()).unwrap();
        std::fs::write(&client_key_path, client_key.serialize_pem()).unwrap();

        TestCerts {
            ca_cert: ca_cert_path,
            client_cert: client_cert_path,
            client_key: client_key_path,
            daemon_cert: daemon_cert_path,
            daemon_key: daemon_key_path,
        }
    }

    async fn wait_until_ready(client: &reqwest::Client, addr: SocketAddr) {
        for _ in 0..40 {
            if client
                .post(format!("https://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("daemon did not become ready");
    }
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-daemon target_info_is_available_over_mutual_tls -- --exact`
Expected: FAIL because `remote_exec_daemon::run` and the daemon routes do not exist yet

- [ ] **Step 3: Implement the daemon config, TLS bootstrap, and health routes**

```rust
// crates/remote-exec-daemon/src/config.rs
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonConfig {
    pub target: String,
    pub listen: SocketAddr,
    pub default_workdir: PathBuf,
    pub tls: TlsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert_pem: PathBuf,
    pub key_pem: PathBuf,
    pub ca_pem: PathBuf,
}

impl DaemonConfig {
    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(toml::from_str(&text)?)
    }
}
```

```rust
// crates/remote-exec-daemon/src/lib.rs
pub mod config;
pub mod server;
pub mod tls;

use std::sync::Arc;

use anyhow::Result;
use config::DaemonConfig;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<DaemonConfig>,
    pub daemon_instance_id: String,
}

pub async fn run(config: DaemonConfig) -> Result<()> {
    let state = AppState {
        config: Arc::new(config),
        daemon_instance_id: uuid::Uuid::new_v4().to_string(),
    };
    server::serve(state).await
}
```

```rust
// crates/remote-exec-daemon/src/server.rs
use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use remote_exec_proto::rpc::{HealthCheckResponse, TargetInfoResponse};

use crate::AppState;

pub async fn serve(state: AppState) -> Result<()> {
    let app = router(Arc::new(state));
    crate::tls::serve_tls(app, Arc::new(state)).await
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .with_state(state)
}

async fn health(State(state): State<Arc<AppState>>) -> Json<HealthCheckResponse> {
    Json(HealthCheckResponse {
        status: "ok".to_string(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
    })
}

async fn target_info(State(state): State<Arc<AppState>>) -> Json<TargetInfoResponse> {
    Json(TargetInfoResponse {
        target: state.config.target.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_instance_id: state.daemon_instance_id.clone(),
        hostname: gethostname::gethostname().to_string_lossy().into_owned(),
        platform: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        supports_pty: true,
        supports_image_read: true,
    })
}
```

```rust
// crates/remote-exec-daemon/src/tls.rs
use std::sync::Arc;

use anyhow::Context;
use axum::Router;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::AppState;

pub async fn serve_tls(app: Router, state: Arc<AppState>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(state.config.listen).await?;
    let tls = TlsAcceptor::from(Arc::new(server_config(&state)?));
    loop {
        let (stream, _) = listener.accept().await?;
        let tls = tls.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let stream = match tls.accept(stream).await {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::warn!(?err, "tls accept failed");
                    return;
                }
            };
            let io = TokioIo::new(stream);
            let service = app.clone().into_service();
            if let Err(err) = http1::Builder::new().serve_connection(io, service).await {
                tracing::warn!(?err, "http serve failed");
            }
        });
    }
}

fn server_config(state: &AppState) -> anyhow::Result<ServerConfig> {
    let certs = load_certs(&state.config.tls.cert_pem)?;
    let key = load_key(&state.config.tls.key_pem)?;
    let client_roots = load_roots(&state.config.tls.ca_pem)?;
    Ok(ServerConfig::builder()
        .with_client_cert_verifier(rustls::server::WebPkiClientVerifier::builder(Arc::new(client_roots)).build()?)
        .with_single_cert(certs, key)?)
}

fn load_certs(path: &std::path::Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let mut pem = std::io::BufReader::new(std::fs::File::open(path)?);
    Ok(rustls_pemfile::certs(&mut pem).collect::<Result<Vec<_>, _>>()?)
}

fn load_key(path: &std::path::Path) -> anyhow::Result<PrivateKeyDer<'static>> {
    let mut pem = std::io::BufReader::new(std::fs::File::open(path)?);
    rustls_pemfile::private_key(&mut pem)?
        .context("missing private key")
}

fn load_roots(path: &std::path::Path) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in load_certs(path)? {
        roots.add(cert)?;
    }
    Ok(roots)
}
```

```rust
// crates/remote-exec-daemon/src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let config = remote_exec_daemon::config::DaemonConfig::load(std::env::args().nth(1).expect("config path")).await?;
    remote_exec_daemon::run(config).await
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon target_info_is_available_over_mutual_tls -- --exact`
Expected: PASS with one successful test and no TLS handshake errors

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon
git commit -m "feat: add daemon health and target info endpoints"
```

### Task 3: Implement `ExecStart` And `ExecWrite` In The Daemon

**Files:**
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Create: `crates/remote-exec-daemon/src/exec/mod.rs`
- Create: `crates/remote-exec-daemon/src/exec/session.rs`
- Create: `crates/remote-exec-daemon/src/exec/transcript.rs`
- Create: `crates/remote-exec-daemon/tests/exec_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon exec_rpc -- --nocapture`

**Testing approach:** `TDD`
Reason: session routing, PTY gating, and exit cleanup are clear observable behaviors with focused integration seams.

- [ ] **Step 1: Write the failing exec RPC tests**

```rust
// crates/remote-exec-daemon/tests/exec_rpc.rs
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest};

#[tokio::test]
async fn exec_start_returns_a_live_session_for_long_running_tty_processes() {
    let fixture = test_support::spawn_daemon("builder-a").await;
    let response = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "printf ready; sleep 2".to_string(),
                workdir: None,
                shell: Some("/bin/bash".to_string()),
                tty: true,
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
                login: Some(false),
            },
        )
        .await;

    assert!(response.running);
    assert!(response.daemon_session_id.is_some());
    assert!(response.output.contains("ready"));
}

#[tokio::test]
async fn exec_write_rejects_non_tty_sessions_when_chars_are_present() {
    let fixture = test_support::spawn_daemon("builder-a").await;
    let started = fixture
        .rpc::<ExecStartRequest, ExecResponse>(
            "/v1/exec/start",
            &ExecStartRequest {
                cmd: "sleep 1".to_string(),
                workdir: None,
                shell: Some("/bin/bash".to_string()),
                tty: false,
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
                login: Some(false),
            },
        )
        .await;

    let session_id = started.daemon_session_id.expect("live session");
    let err = fixture
        .rpc_error(
            "/v1/exec/write",
            &ExecWriteRequest {
                daemon_session_id: session_id,
                chars: "pwd\n".to_string(),
                yield_time_ms: Some(250),
                max_output_tokens: Some(2_000),
            },
        )
        .await;

    assert_eq!(err.code, "stdin_closed");
    assert!(err.message.contains("tty=true"));
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-daemon exec_start_returns_a_live_session_for_long_running_tty_processes -- --exact`
Expected: FAIL with `404 Not Found` because `/v1/exec/start` is not registered yet

- [ ] **Step 3: Implement the session store, PTY logic, and RPC handlers**

```rust
// crates/remote-exec-daemon/src/lib.rs
pub mod config;
pub mod exec;
pub mod server;
pub mod tls;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use config::DaemonConfig;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<DaemonConfig>,
    pub daemon_instance_id: String,
    pub sessions: Arc<RwLock<HashMap<String, exec::session::LiveSession>>>,
}
```

```rust
// crates/remote-exec-daemon/src/exec/transcript.rs
#[derive(Debug, Clone)]
pub struct TranscriptBuffer {
    limit: usize,
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
}

impl TranscriptBuffer {
    pub fn new(limit: usize) -> Self {
        Self { limit, head: Vec::new(), tail: Vec::new(), total: 0 }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        let head_limit = self.limit / 2;
        let tail_limit = self.limit - head_limit;
        if self.head.len() < head_limit {
            let take = (head_limit - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..take]);
        }
        self.tail.extend_from_slice(bytes);
        if self.tail.len() > tail_limit {
            let drop_len = self.tail.len() - tail_limit;
            self.tail.drain(..drop_len);
        }
    }

    pub fn render(&self) -> String {
        let mut data = self.head.clone();
        if self.total > self.limit {
            data.extend_from_slice(b"\n...<truncated>...\n");
        }
        data.extend_from_slice(&self.tail);
        String::from_utf8_lossy(&data).into_owned()
    }
}
```

```rust
// crates/remote-exec-daemon/src/exec/session.rs
use std::io::{Read, Write};
use std::process::Stdio;
use std::time::Instant;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::transcript::TranscriptBuffer;

pub struct LiveSession {
    pub tty: bool,
    pub started_at: Instant,
    pub transcript: TranscriptBuffer,
    pub child: SessionChild,
}

pub enum SessionChild {
    Pty(PtySession),
    Pipe(tokio::process::Child),
}

pub struct PtySession {
    pub child: Box<dyn portable_pty::Child + Send + Sync>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub reader: Box<dyn std::io::Read + Send>,
}

pub fn spawn(cmd: &[String], cwd: &std::path::Path, tty: bool) -> anyhow::Result<LiveSession> {
    if tty {
        let pty = NativePtySystem::default().openpty(PtySize { rows: 24, cols: 120, pixel_width: 0, pixel_height: 0 })?;
        let mut builder = CommandBuilder::new(&cmd[0]);
        for arg in &cmd[1..] {
            builder.arg(arg);
        }
        builder.cwd(cwd);
        let child = pty.slave.spawn_command(builder)?;
        let writer = pty.master.take_writer()?;
        let reader = pty.master.try_clone_reader()?;
        Ok(LiveSession {
            tty: true,
            started_at: Instant::now(),
            transcript: TranscriptBuffer::new(1024 * 1024),
            child: SessionChild::Pty(PtySession { child, writer, reader }),
        })
    } else {
        let mut command = Command::new(&cmd[0]);
        command.args(&cmd[1..]).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command.spawn()?;
        Ok(LiveSession {
            tty: false,
            started_at: Instant::now(),
            transcript: TranscriptBuffer::new(1024 * 1024),
            child: SessionChild::Pipe(child),
        })
    }
}

impl LiveSession {
    pub async fn read_available(&mut self) -> anyhow::Result<String> {
        match &mut self.child {
            SessionChild::Pty(pty) => {
                let mut buffer = [0u8; 8192];
                let read = pty.reader.read(&mut buffer).unwrap_or(0);
                Ok(String::from_utf8_lossy(&buffer[..read]).into_owned())
            }
            SessionChild::Pipe(child) => {
                let mut output = String::new();
                if let Some(stdout) = child.stdout.as_mut() {
                    let mut buffer = [0u8; 8192];
                    if let Ok(Ok(read)) = tokio::time::timeout(std::time::Duration::from_millis(10), stdout.read(&mut buffer)).await {
                        output.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    }
                }
                if let Some(stderr) = child.stderr.as_mut() {
                    let mut buffer = [0u8; 8192];
                    if let Ok(Ok(read)) = tokio::time::timeout(std::time::Duration::from_millis(10), stderr.read(&mut buffer)).await {
                        output.push_str(&String::from_utf8_lossy(&buffer[..read]));
                    }
                }
                Ok(output)
            }
        }
    }

    pub async fn has_exited(&mut self) -> anyhow::Result<bool> {
        match &mut self.child {
            SessionChild::Pty(pty) => Ok(pty.child.try_wait()?.is_some()),
            SessionChild::Pipe(child) => Ok(child.try_wait()?.is_some()),
        }
    }

    pub async fn write(&mut self, chars: &str) -> anyhow::Result<()> {
        if chars.is_empty() {
            return Ok(());
        }
        match &mut self.child {
            SessionChild::Pty(pty) => {
                pty.writer.write_all(chars.as_bytes())?;
                pty.writer.flush()?;
                Ok(())
            }
            SessionChild::Pipe(_) => anyhow::bail!("stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"),
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        None
    }
}
```

```rust
// crates/remote-exec-daemon/src/exec/mod.rs
pub mod session;
pub mod transcript;

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use remote_exec_proto::rpc::{ExecResponse, ExecStartRequest, ExecWriteRequest, RpcErrorBody};

use crate::AppState;

pub async fn exec_start(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecStartRequest>,
) -> Result<Json<ExecResponse>, (axum::http::StatusCode, Json<RpcErrorBody>)> {
    let cwd = resolve_workdir(&state, req.workdir.as_deref())?;
    let argv = shell_argv(req.shell.as_deref(), req.login.unwrap_or(false), &req.cmd);
    let mut session = session::spawn(&argv, &cwd, req.tty).map_err(internal_error)?;
    let daemon_session_id = uuid::Uuid::new_v4().to_string();
    let deadline = Instant::now() + Duration::from_millis(req.yield_time_ms.unwrap_or(10_000).clamp(250, 30_000));
    let mut output = String::new();
    while Instant::now() < deadline {
        let chunk = poll_once(&mut session).await.map_err(internal_error)?;
        if !chunk.is_empty() {
            output.push_str(&chunk);
            session.transcript.push(chunk.as_bytes());
        }
        if has_exited(&mut session).await.map_err(internal_error)? {
            return Ok(Json(finish_response(None, false, &session, output)));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    state.sessions.write().await.insert(daemon_session_id.clone(), session);
    Ok(Json(ExecResponse {
        daemon_session_id: Some(daemon_session_id),
        running: true,
        chunk_id: Some(chunk_id()),
        wall_time_seconds: 0.25,
        exit_code: None,
        original_token_count: Some(output.split_whitespace().count() as u32),
        output,
    }))
}

pub async fn exec_write(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ExecWriteRequest>,
) -> Result<Json<ExecResponse>, (axum::http::StatusCode, Json<RpcErrorBody>)> {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&req.daemon_session_id).ok_or_else(|| rpc_error("unknown_session", "Unknown daemon session"))?;
    if !req.chars.is_empty() && !session.tty {
        return Err(rpc_error("stdin_closed", "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"));
    }
    write_chars(session, &req.chars).await.map_err(internal_error)?;
    let output = poll_until(session, req.chars.is_empty(), req.yield_time_ms.unwrap_or(250)).await.map_err(internal_error)?;
    if has_exited(session).await.map_err(internal_error)? {
        sessions.remove(&req.daemon_session_id);
        return Ok(Json(finish_response(None, false, session, output)));
    }
    Ok(Json(ExecResponse {
        daemon_session_id: Some(req.daemon_session_id),
        running: true,
        chunk_id: Some(chunk_id()),
        wall_time_seconds: 0.25,
        exit_code: None,
        original_token_count: Some(output.split_whitespace().count() as u32),
        output,
    }))
}

pub fn resolve_workdir(state: &Arc<AppState>, workdir: Option<&str>) -> anyhow::Result<std::path::PathBuf> {
    Ok(match workdir {
        None => state.config.default_workdir.clone(),
        Some(raw) => {
            let path = std::path::PathBuf::from(raw);
            if path.is_absolute() {
                path
            } else {
                state.config.default_workdir.join(path)
            }
        }
    })
}

pub fn rpc_error(code: &'static str, message: impl Into<String>) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::BAD_REQUEST,
        Json(RpcErrorBody {
            code,
            message: message.into(),
        }),
    )
}

pub fn internal_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(RpcErrorBody {
            code: "internal_error",
            message: err.to_string(),
        }),
    )
}

fn shell_argv(shell: Option<&str>, login: bool, cmd: &str) -> Vec<String> {
    let shell = shell.unwrap_or("/bin/bash");
    let mode = if login { "-lc" } else { "-c" };
    vec![shell.to_string(), mode.to_string(), cmd.to_string()]
}

fn chunk_id() -> String {
    let bytes: [u8; 3] = rand::random();
    format!("{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

async fn poll_once(session: &mut session::LiveSession) -> anyhow::Result<String> {
    session.read_available().await
}

async fn has_exited(session: &mut session::LiveSession) -> anyhow::Result<bool> {
    session.has_exited().await
}

async fn write_chars(session: &mut session::LiveSession, chars: &str) -> anyhow::Result<()> {
    session.write(chars).await
}

async fn poll_until(session: &mut session::LiveSession, empty_poll: bool, requested_ms: u64) -> anyhow::Result<String> {
    let lower = if empty_poll { 5_000 } else { 250 };
    let upper = if empty_poll { 300_000 } else { 30_000 };
    let deadline = Instant::now() + Duration::from_millis(requested_ms.clamp(lower, upper));
    let mut output = String::new();
    while Instant::now() < deadline {
        let chunk = poll_once(session).await?;
        if !chunk.is_empty() {
            session.transcript.push(chunk.as_bytes());
            output.push_str(&chunk);
        }
        if has_exited(session).await? {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Ok(output)
}

fn finish_response(daemon_session_id: Option<String>, running: bool, session: &session::LiveSession, output: String) -> ExecResponse {
    ExecResponse {
        daemon_session_id,
        running,
        chunk_id: Some(chunk_id()),
        wall_time_seconds: session.started_at.elapsed().as_secs_f64(),
        exit_code: session.exit_code(),
        original_token_count: Some(output.split_whitespace().count() as u32),
        output,
    }
}
```

```rust
// crates/remote-exec-daemon/src/server.rs
Router::new()
    .route("/v1/health", post(health))
    .route("/v1/target-info", post(target_info))
    .route("/v1/exec/start", post(crate::exec::exec_start))
    .route("/v1/exec/write", post(crate::exec::exec_write))
    .with_state(state)
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon exec_rpc -- --nocapture`
Expected: PASS with both exec RPC tests green

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon
git commit -m "feat: add daemon exec session RPCs"
```

### Task 4: Implement `PatchApply` In The Daemon

**Files:**
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Create: `crates/remote-exec-daemon/src/patch/mod.rs`
- Create: `crates/remote-exec-daemon/src/patch/parser.rs`
- Create: `crates/remote-exec-daemon/tests/patch_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon patch_rpc -- --nocapture`

**Testing approach:** `characterization/integration test`
Reason: the documented behavior comes from existing tool semantics, so characterization tests should pin down overwrite and partial-failure behavior.

- [ ] **Step 1: Write the failing patch RPC tests**

```rust
// crates/remote-exec-daemon/tests/patch_rpc.rs
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse};

#[tokio::test]
async fn add_file_overwrites_existing_content() {
    let fixture = test_support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("demo.txt");
    tokio::fs::write(&path, "old\n").await.unwrap();

    let response = fixture
        .rpc::<PatchApplyRequest, PatchApplyResponse>(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: "*** Begin Patch\n*** Add File: demo.txt\n+new\n*** End Patch\n".to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert!(response.output.contains("Success."));
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "new\n");
}

#[tokio::test]
async fn patch_failures_do_not_roll_back_earlier_file_changes() {
    let fixture = test_support::spawn_daemon("builder-a").await;
    tokio::fs::write(fixture.workdir.join("first.txt"), "before\n").await.unwrap();

    let err = fixture
        .rpc_error(
            "/v1/patch/apply",
            &PatchApplyRequest {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Update File: first.txt\n",
                    "@@\n",
                    "-before\n",
                    "+after\n",
                    "*** Delete File: missing.txt\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some(".".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "patch_failed");
    assert_eq!(
        tokio::fs::read_to_string(fixture.workdir.join("first.txt")).await.unwrap(),
        "after\n",
    );
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-daemon add_file_overwrites_existing_content -- --exact`
Expected: FAIL with `404 Not Found` because `/v1/patch/apply` is not registered yet

- [ ] **Step 3: Implement the patch parser, verifier, and applier**

```rust
// crates/remote-exec-daemon/src/patch/parser.rs
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchAction {
    Add { path: PathBuf, lines: Vec<String> },
    Delete { path: PathBuf },
    Update { path: PathBuf, move_to: Option<PathBuf>, hunks: Vec<Hunk> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub context: Option<String>,
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HunkLine {
    Context(String),
    Delete(String),
    Add(String),
}

pub fn parse_patch(input: &str) -> anyhow::Result<Vec<PatchAction>> {
    let lines: Vec<&str> = input.lines().collect();
    anyhow::ensure!(lines.first() == Some(&"*** Begin Patch"), "invalid patch header");
    anyhow::ensure!(lines.last() == Some(&"*** End Patch"), "invalid patch footer");

    let mut actions = Vec::new();
    let mut index = 1;
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut added = Vec::new();
            while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
                let raw = lines[index];
                let value = raw
                    .strip_prefix('+')
                    .ok_or_else(|| anyhow::anyhow!("add file lines must start with `+`"))?;
                added.push(value.to_string());
                index += 1;
            }
            actions.push(PatchAction::Add {
                path: path.into(),
                lines: added,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            actions.push(PatchAction::Delete { path: path.into() });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if index + 1 < lines.len() {
                if let Some(destination) = lines[index].strip_prefix("*** Move to: ") {
                    move_to = Some(destination.into());
                    index += 1;
                }
            }

            let mut hunks = Vec::new();
            while index + 1 < lines.len() && !lines[index].starts_with("*** ") {
                let header = lines[index];
                let context = if header == "@@" {
                    None
                } else if let Some(rest) = header.strip_prefix("@@ ") {
                    Some(rest.to_string())
                } else {
                    anyhow::bail!("invalid update hunk header `{header}`");
                };
                index += 1;

                let mut hunk_lines = Vec::new();
                while index + 1 < lines.len()
                    && !lines[index].starts_with("@@")
                    && !lines[index].starts_with("*** ")
                {
                    let raw = lines[index];
                    let parsed = match raw.chars().next() {
                        Some(' ') => HunkLine::Context(raw[1..].to_string()),
                        Some('-') => HunkLine::Delete(raw[1..].to_string()),
                        Some('+') => HunkLine::Add(raw[1..].to_string()),
                        _ => anyhow::bail!("invalid update hunk line `{raw}`"),
                    };
                    hunk_lines.push(parsed);
                    index += 1;
                }
                anyhow::ensure!(!hunk_lines.is_empty(), "update hunk with no changes");
                hunks.push(Hunk { context, lines: hunk_lines });
            }

            actions.push(PatchAction::Update {
                path: path.into(),
                move_to,
                hunks,
            });
            continue;
        }

        anyhow::bail!("unsupported patch line `{line}`");
    }
    Ok(actions)
}
```

```rust
// crates/remote-exec-daemon/src/patch/mod.rs
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use remote_exec_proto::rpc::{PatchApplyRequest, PatchApplyResponse, RpcErrorBody};

use crate::AppState;

pub async fn apply_patch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (axum::http::StatusCode, Json<RpcErrorBody>)> {
    let cwd = super::exec::resolve_workdir(&state, req.workdir.as_deref()).map_err(super::exec::internal_error)?;
    let actions = parser::parse_patch(&req.patch).map_err(|err| super::exec::rpc_error("patch_failed", err.to_string()))?;
    let mut summary = Vec::new();

    for action in actions {
        match action {
            parser::PatchAction::Add { path, lines } => {
                let path = cwd.join(path);
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(super::exec::internal_error)?;
                }
                tokio::fs::write(&path, format!("{}\n", lines.join("\n"))).await.map_err(super::exec::internal_error)?;
                summary.push(format!("A {}", display_relative(&cwd, &path)));
            }
            parser::PatchAction::Delete { path } => {
                let path = cwd.join(path);
                tokio::fs::remove_file(&path).await.map_err(|err| super::exec::rpc_error("patch_failed", err.to_string()))?;
                summary.push(format!("D {}", display_relative(&cwd, &path)));
            }
            parser::PatchAction::Update { path, move_to, hunks } => {
                let path = cwd.join(path);
                let current = tokio::fs::read_to_string(&path).await.map_err(|err| super::exec::rpc_error("patch_failed", err.to_string()))?;
                let updated = apply_hunks(&current, &hunks).map_err(|err| super::exec::rpc_error("patch_failed", err.to_string()))?;
                if let Some(move_to) = move_to {
                    let destination = cwd.join(move_to);
                    if let Some(parent) = destination.parent() {
                        tokio::fs::create_dir_all(parent).await.map_err(super::exec::internal_error)?;
                    }
                    tokio::fs::write(&destination, ensure_trailing_newline(updated)).await.map_err(super::exec::internal_error)?;
                    tokio::fs::remove_file(&path).await.map_err(super::exec::internal_error)?;
                    summary.push(format!("M {}", display_relative(&cwd, &destination)));
                } else {
                    tokio::fs::write(&path, ensure_trailing_newline(updated)).await.map_err(super::exec::internal_error)?;
                    summary.push(format!("M {}", display_relative(&cwd, &path)));
                }
            }
        }
    }

    Ok(Json(PatchApplyResponse {
        output: format!("Success. Updated the following files:\n{}\n", summary.join("\n")),
    }))
}

fn ensure_trailing_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn display_relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base).unwrap_or(path).display().to_string()
}

fn apply_hunks(current: &str, hunks: &[parser::Hunk]) -> anyhow::Result<String> {
    let mut lines = current.lines().map(str::to_string).collect::<Vec<_>>();
    for hunk in hunks {
        let anchor = hunk
            .context
            .as_ref()
            .and_then(|ctx| lines.iter().position(|line| line.contains(ctx)));
        let mut cursor = anchor.unwrap_or(0);
        let mut replacement = Vec::new();

        for line in &hunk.lines {
            match line {
                parser::HunkLine::Context(value) => {
                    let found = lines
                        .iter()
                        .enumerate()
                        .skip(cursor)
                        .find(|(_, line)| *line == value)
                        .map(|(index, _)| index)
                        .ok_or_else(|| anyhow::anyhow!("context line `{value}` not found"))?;
                    replacement.extend(lines[cursor..=found].iter().cloned());
                    cursor = found + 1;
                }
                parser::HunkLine::Delete(value) => {
                    let found = lines
                        .iter()
                        .enumerate()
                        .skip(cursor)
                        .find(|(_, line)| *line == value)
                        .map(|(index, _)| index)
                        .ok_or_else(|| anyhow::anyhow!("delete line `{value}` not found"))?;
                    replacement.extend(lines[cursor..found].iter().cloned());
                    cursor = found + 1;
                }
                parser::HunkLine::Add(value) => replacement.push(value.clone()),
            }
        }

        replacement.extend(lines[cursor..].iter().cloned());
        lines = replacement;
    }
    Ok(lines.join("\n"))
}
```

```rust
// crates/remote-exec-daemon/src/server.rs
Router::new()
    .route("/v1/health", post(health))
    .route("/v1/target-info", post(target_info))
    .route("/v1/exec/start", post(crate::exec::exec_start))
    .route("/v1/exec/write", post(crate::exec::exec_write))
    .route("/v1/patch/apply", post(crate::patch::apply_patch))
    .with_state(state)
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon patch_rpc -- --nocapture`
Expected: PASS with overwrite and partial-failure behavior pinned by tests

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon
git commit -m "feat: add daemon patch apply RPC"
```

### Task 5: Implement `ImageRead` In The Daemon

**Files:**
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Create: `crates/remote-exec-daemon/src/image.rs`
- Create: `crates/remote-exec-daemon/tests/image_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon image_rpc -- --nocapture`

**Testing approach:** `characterization/integration test`
Reason: image semantics are easiest to validate from actual file fixtures and returned payloads.

- [ ] **Step 1: Write the failing image RPC tests**

```rust
// crates/remote-exec-daemon/tests/image_rpc.rs
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse};

#[tokio::test]
async fn image_read_resizes_large_images_by_default() {
    let fixture = test_support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("large.png");
    test_support::write_png(&path, 4096, 2048).await;

    let response = fixture
        .rpc::<ImageReadRequest, ImageReadResponse>(
            "/v1/image/read",
            &ImageReadRequest {
                path: "large.png".to_string(),
                workdir: Some(".".to_string()),
                detail: None,
            },
        )
        .await;

    assert!(response.image_url.starts_with("data:image/png;base64,"));
    assert_eq!(response.detail, None);
}

#[tokio::test]
async fn image_read_rejects_unknown_detail_values() {
    let fixture = test_support::spawn_daemon("builder-a").await;
    let path = fixture.workdir.join("small.png");
    test_support::write_png(&path, 32, 32).await;

    let err = fixture
        .rpc_error(
            "/v1/image/read",
            &ImageReadRequest {
                path: "small.png".to_string(),
                workdir: Some(".".to_string()),
                detail: Some("low".to_string()),
            },
        )
        .await;

    assert_eq!(err.code, "invalid_detail");
    assert!(err.message.contains("original"));
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-daemon image_read_resizes_large_images_by_default -- --exact`
Expected: FAIL with `404 Not Found` because `/v1/image/read` is not registered yet

- [ ] **Step 3: Implement file validation, resize-to-fit, and data URL encoding**

```rust
// crates/remote-exec-daemon/src/image.rs
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use base64::Engine;
use image::ImageFormat;
use remote_exec_proto::rpc::{ImageReadRequest, ImageReadResponse, RpcErrorBody};

use crate::AppState;

const MAX_WIDTH: u32 = 2048;
const MAX_HEIGHT: u32 = 768;

pub async fn read_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ImageReadRequest>,
) -> Result<Json<ImageReadResponse>, (axum::http::StatusCode, Json<RpcErrorBody>)> {
    match req.detail.as_deref() {
        None | Some("original") => {}
        Some(other) => return Err(crate::exec::rpc_error("invalid_detail", format!("view_image.detail only supports `original`; got `{other}`"))),
    }

    let cwd = crate::exec::resolve_workdir(&state, req.workdir.as_deref()).map_err(crate::exec::internal_error)?;
    let path = cwd.join(&req.path);
    let metadata = tokio::fs::metadata(&path).await.map_err(|err| crate::exec::rpc_error("image_missing", err.to_string()))?;
    if !metadata.is_file() {
        return Err(crate::exec::rpc_error("image_not_file", format!("image path `{}` is not a file", path.display())));
    }

    let bytes = tokio::fs::read(&path).await.map_err(crate::exec::internal_error)?;
    let format = image::guess_format(&bytes).map_err(|err| crate::exec::rpc_error("image_decode_failed", err.to_string()))?;
    let payload = if req.detail.as_deref() == Some("original") {
        encode_data_url(format, bytes)?
    } else {
        let image = image::load_from_memory(&bytes).map_err(|err| crate::exec::rpc_error("image_decode_failed", err.to_string()))?;
        let resized = image.resize(MAX_WIDTH, MAX_HEIGHT, image::imageops::FilterType::Triangle);
        let mut out = Vec::new();
        resized
            .write_to(&mut std::io::Cursor::new(&mut out), ImageFormat::Png)
            .map_err(|err| crate::exec::rpc_error("image_encode_failed", err.to_string()))?;
        encode_data_url(ImageFormat::Png, out)?
    };

    Ok(Json(ImageReadResponse { image_url: payload, detail: req.detail.filter(|value| value == "original") }))
}

fn encode_data_url(format: ImageFormat, bytes: Vec<u8>) -> Result<String, (axum::http::StatusCode, Json<RpcErrorBody>)> {
    let mime = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        other => return Err(crate::exec::rpc_error("image_decode_failed", format!("unsupported image format `{other:?}`"))),
    };
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}
```

```rust
// crates/remote-exec-daemon/src/server.rs
Router::new()
    .route("/v1/health", post(health))
    .route("/v1/target-info", post(target_info))
    .route("/v1/exec/start", post(crate::exec::exec_start))
    .route("/v1/exec/write", post(crate::exec::exec_write))
    .route("/v1/patch/apply", post(crate::patch::apply_patch))
    .route("/v1/image/read", post(crate::image::read_image))
    .with_state(state)
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon image_rpc -- --nocapture`
Expected: PASS with default resize and invalid-detail validation both green

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon
git commit -m "feat: add daemon image read RPC"
```

### Task 6: Build The Broker Core, Daemon Client, And Exec MCP Tools

**Files:**
- Create: `crates/remote-exec-broker/src/lib.rs`
- Modify: `crates/remote-exec-broker/src/main.rs`
- Create: `crates/remote-exec-broker/src/config.rs`
- Create: `crates/remote-exec-broker/src/session_store.rs`
- Create: `crates/remote-exec-broker/src/daemon_client.rs`
- Create: `crates/remote-exec-broker/src/mcp_server.rs`
- Create: `crates/remote-exec-broker/src/tools/mod.rs`
- Create: `crates/remote-exec-broker/src/tools/exec.rs`
- Create: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Test/Verify: `cargo test -p remote-exec-broker mcp_exec -- --nocapture`

**Testing approach:** `TDD`
Reason: broker session routing and public tool contracts have a clear request/response seam and should be pinned before implementation.

- [ ] **Step 1: Write the failing broker exec MCP tests**

```rust
// crates/remote-exec-broker/tests/mcp_exec.rs
use remote_exec_proto::public::CommandToolResult;

#[tokio::test]
async fn exec_command_returns_an_opaque_string_session_id() {
    let fixture = test_support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf ready; sleep 2",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    let structured: CommandToolResult = serde_json::from_value(result.structured_content).unwrap();
    let session_id = structured.session_id.expect("running session");
    assert!(session_id.starts_with("sess_"));
    assert!(structured.exit_code.is_none());
}

#[tokio::test]
async fn write_stdin_routes_by_public_session_id_instead_of_target_guessing() {
    let fixture = test_support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": "sess_test_1",
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;

    let structured: CommandToolResult = serde_json::from_value(result.structured_content).unwrap();
    assert_eq!(structured.target, "builder-a");
    assert!(structured.output.contains("poll output"));
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-broker exec_command_returns_an_opaque_string_session_id -- --exact`
Expected: FAIL because the broker MCP server and tool registration do not exist yet

- [ ] **Step 3: Implement broker config loading, daemon RPC client, session store, and exec MCP tools**

```rust
// crates/remote-exec-broker/src/config.rs
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct BrokerConfig {
    pub targets: BTreeMap<String, TargetConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TargetConfig {
    pub base_url: String,
    pub ca_pem: PathBuf,
    pub client_cert_pem: PathBuf,
    pub client_key_pem: PathBuf,
    pub expected_daemon_name: Option<String>,
}

impl BrokerConfig {
    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        Ok(toml::from_str(&text)?)
    }
}
```

```rust
// crates/remote-exec-broker/src/session_store.rs
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub target: String,
    pub daemon_session_id: String,
    pub daemon_instance_id: String,
}

#[derive(Default, Clone)]
pub struct SessionStore {
    inner: Arc<RwLock<HashMap<String, SessionRecord>>>,
}

impl SessionStore {
    pub async fn insert(&self, target: String, daemon_session_id: String, daemon_instance_id: String) -> SessionRecord {
        let session_id = format!("sess_{}", uuid::Uuid::new_v4().simple());
        let record = SessionRecord { session_id: session_id.clone(), target, daemon_session_id, daemon_instance_id };
        self.inner.write().await.insert(session_id.clone(), record.clone());
        record
    }

    pub async fn get(&self, session_id: &str) -> Option<SessionRecord> {
        self.inner.read().await.get(session_id).cloned()
    }

    pub async fn remove(&self, session_id: &str) {
        self.inner.write().await.remove(session_id);
    }
}
```

```rust
// crates/remote-exec-broker/src/daemon_client.rs
use anyhow::Context;
use remote_exec_proto::rpc::*;
use reqwest::Identity;
use reqwest::tls::Certificate;

use crate::config::TargetConfig;

#[derive(Clone)]
pub struct DaemonClient {
    client: reqwest::Client,
    base_url: String,
}

impl DaemonClient {
    pub async fn new(config: &TargetConfig) -> anyhow::Result<Self> {
        let ca = Certificate::from_pem(&tokio::fs::read(&config.ca_pem).await?)?;
        let identity = Identity::from_pem(&[
            tokio::fs::read(&config.client_cert_pem).await?,
            tokio::fs::read(&config.client_key_pem).await?,
        ]
        .concat())?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(ca)
            .identity(identity)
            .build()
            .context("building daemon client")?;
        Ok(Self { client, base_url: config.base_url.clone() })
    }

    pub async fn exec_start(&self, req: &ExecStartRequest) -> anyhow::Result<ExecResponse> {
        self.post("/v1/exec/start", req).await
    }

    pub async fn exec_write(&self, req: &ExecWriteRequest) -> anyhow::Result<ExecResponse> {
        self.post("/v1/exec/write", req).await
    }

    async fn post<Req, Resp>(&self, path: &str, body: &Req) -> anyhow::Result<Resp>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        Ok(self.client.post(format!("{}{}", self.base_url, path)).json(body).send().await?.error_for_status()?.json().await?)
    }
}
```

```rust
// crates/remote-exec-broker/src/tools/exec.rs
use anyhow::Context;
use remote_exec_proto::public::{CommandToolResult, ExecCommandInput, WriteStdinInput};
use remote_exec_proto::rpc::{ExecStartRequest, ExecWriteRequest};

use crate::mcp_server::{format_command_text, format_poll_text, ToolCallOutput};

pub async fn exec_command(state: &crate::BrokerState, input: ExecCommandInput) -> anyhow::Result<ToolCallOutput> {
    let target = state.target(&input.target)?;
    let response = target
        .client
        .exec_start(&ExecStartRequest {
            cmd: input.cmd.clone(),
            workdir: input.workdir.clone(),
            shell: input.shell.clone(),
            tty: input.tty,
            yield_time_ms: input.yield_time_ms,
            max_output_tokens: input.max_output_tokens,
            login: input.login,
        })
        .await?;
    let session_id = if response.running {
        let daemon_session_id = response.daemon_session_id.clone().expect("daemon session id");
        Some(state.sessions.insert(input.target.clone(), daemon_session_id, target.daemon_instance_id.clone()).await.session_id)
    } else {
        None
    };
    Ok(ToolCallOutput::text_and_structured(
        format_command_text(&input.cmd, &response, session_id.as_deref()),
        serde_json::to_value(CommandToolResult {
            target: input.target,
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            exit_code: response.exit_code,
            session_id,
            original_token_count: response.original_token_count,
            output: response.output,
        })?,
    ))
}

pub async fn write_stdin(state: &crate::BrokerState, input: WriteStdinInput) -> anyhow::Result<ToolCallOutput> {
    let record = state.sessions.get(&input.session_id).await.context("unknown session")?;
    if let Some(target) = &input.target {
        anyhow::ensure!(target == &record.target, "session does not belong to target `{target}`");
    }
    let target = state.target(&record.target)?;
    let response = match target
        .client
        .exec_write(&ExecWriteRequest {
            daemon_session_id: record.daemon_session_id.clone(),
            chars: input.chars.unwrap_or_default(),
            yield_time_ms: input.yield_time_ms,
            max_output_tokens: input.max_output_tokens,
        })
        .await
    {
        Ok(response) => response,
        Err(err) => {
            state.sessions.remove(&record.session_id).await;
            return Err(err.context("session invalidated after daemon-side session loss"));
        }
    };
    let session_id = if response.running {
        Some(record.session_id.clone())
    } else {
        state.sessions.remove(&record.session_id).await;
        None
    };
    Ok(ToolCallOutput::text_and_structured(
        format_poll_text(&response, session_id.as_deref()),
        serde_json::to_value(CommandToolResult {
            target: record.target,
            chunk_id: response.chunk_id,
            wall_time_seconds: response.wall_time_seconds,
            exit_code: response.exit_code,
            session_id,
            original_token_count: response.original_token_count,
            output: response.output,
        })?,
    ))
}
```

```rust
// crates/remote-exec-broker/src/lib.rs
pub mod config;
pub mod daemon_client;
pub mod mcp_server;
pub mod session_store;
pub mod tools;

use std::collections::BTreeMap;

use anyhow::Context;
use daemon_client::DaemonClient;
use session_store::SessionStore;

#[derive(Clone)]
pub struct TargetHandle {
    pub client: DaemonClient,
    pub daemon_instance_id: String,
}

#[derive(Clone)]
pub struct BrokerState {
    pub sessions: SessionStore,
    pub targets: BTreeMap<String, TargetHandle>,
}

impl BrokerState {
    pub fn target(&self, name: &str) -> anyhow::Result<&TargetHandle> {
        self.targets
            .get(name)
            .with_context(|| format!("unknown target `{name}`"))
    }
}
```

```rust
// crates/remote-exec-broker/src/mcp_server.rs
use rmcp::model::CallToolResult;
use rmcp::model::Content;

pub struct ToolCallOutput {
    pub content: Vec<Content>,
    pub structured: serde_json::Value,
}

impl ToolCallOutput {
    pub fn text_and_structured(text: String, structured: serde_json::Value) -> Self {
        Self {
            content: vec![Content::text(text)],
            structured,
        }
    }

    pub fn content_and_structured(content: Vec<Content>, structured: serde_json::Value) -> Self {
        Self { content, structured }
    }

    pub fn into_call_tool_result(self) -> CallToolResult {
        CallToolResult {
            content: self.content,
            structured_content: Some(self.structured),
            is_error: Some(false),
            meta: None,
        }
    }
}

pub fn format_command_text(cmd: &str, response: &remote_exec_proto::rpc::ExecResponse, session_id: Option<&str>) -> String {
    let status = match (response.exit_code, session_id) {
        (Some(code), _) => format!("Process exited with code {code}"),
        (None, Some(id)) => format!("Process running with session ID {id}"),
        (None, None) => "Process running".to_string(),
    };
    format!(
        "Command: {cmd}\nChunk ID: {}\nWall time: {:.3} seconds\n{status}\nOutput:\n{}",
        response.chunk_id.clone().unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        response.output
    )
}

pub fn format_poll_text(response: &remote_exec_proto::rpc::ExecResponse, session_id: Option<&str>) -> String {
    let status = match (response.exit_code, session_id) {
        (Some(code), _) => format!("Process exited with code {code}"),
        (None, Some(id)) => format!("Process running with session ID {id}"),
        (None, None) => "Process running".to_string(),
    };
    format!(
        "Chunk ID: {}\nWall time: {:.3} seconds\n{status}\nOutput:\n{}",
        response.chunk_id.clone().unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        response.output
    )
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker mcp_exec -- --nocapture`
Expected: PASS with broker-generated `sess_` IDs and correct routing on `write_stdin`

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker
git commit -m "feat: add broker exec tools"
```

### Task 7: Implement Broker `apply_patch` And `view_image`

**Files:**
- Create: `crates/remote-exec-broker/src/tools/patch.rs`
- Create: `crates/remote-exec-broker/src/tools/image.rs`
- Modify: `crates/remote-exec-broker/src/tools/mod.rs`
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Create: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Test/Verify: `cargo test -p remote-exec-broker mcp_assets -- --nocapture`

**Testing approach:** `characterization/integration test`
Reason: these tools are mostly response-shape and error-translation work on top of already-characterized daemon behavior.

- [ ] **Step 1: Write the failing asset-tool tests**

```rust
// crates/remote-exec-broker/tests/mcp_assets.rs
use remote_exec_proto::public::{ApplyPatchResult, ViewImageResult};

#[tokio::test]
async fn apply_patch_returns_plain_text_plus_structured_content() {
    let fixture = test_support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: hello.txt\n+hello\n*** End Patch\n",
                "workdir": "."
            }),
        )
        .await;

    assert!(result.text_output.contains("Success. Updated the following files:"));
    let structured: ApplyPatchResult = serde_json::from_value(result.structured_content).unwrap();
    assert_eq!(structured.target, "builder-a");
}

#[tokio::test]
async fn view_image_returns_input_image_content_and_structured_content() {
    let fixture = test_support::spawn_broker_with_stub_daemon().await;
    let result = fixture
        .call_tool(
            "view_image",
            serde_json::json!({
                "target": "builder-a",
                "path": "chart.png",
                "detail": "original"
            }),
        )
        .await;

    assert_eq!(result.image_output["type"], "input_image");
    let structured: ViewImageResult = serde_json::from_value(result.structured_content).unwrap();
    assert_eq!(structured.detail.as_deref(), Some("original"));
}
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-broker apply_patch_returns_plain_text_plus_structured_content -- --exact`
Expected: FAIL because `apply_patch` and `view_image` are not registered with the broker yet

- [ ] **Step 3: Implement the broker patch and image tool modules**

```rust
// crates/remote-exec-broker/src/tools/patch.rs
use remote_exec_proto::public::{ApplyPatchInput, ApplyPatchResult};
use remote_exec_proto::rpc::PatchApplyRequest;

use crate::mcp_server::ToolCallOutput;

pub async fn apply_patch(state: &crate::BrokerState, input: ApplyPatchInput) -> anyhow::Result<ToolCallOutput> {
    let target = state.target(&input.target)?;
    let response = target
        .client
        .patch_apply(&PatchApplyRequest {
            patch: input.input,
            workdir: input.workdir,
        })
        .await?;

    Ok(ToolCallOutput::text_and_structured(
        response.output.clone(),
        serde_json::to_value(ApplyPatchResult {
            target: input.target,
            output: response.output,
        })?,
    ))
}
```

```rust
// crates/remote-exec-broker/src/tools/image.rs
use remote_exec_proto::public::{ViewImageInput, ViewImageResult};
use remote_exec_proto::rpc::ImageReadRequest;
use rmcp::model::Content;

use crate::mcp_server::ToolCallOutput;

pub async fn view_image(state: &crate::BrokerState, input: ViewImageInput) -> anyhow::Result<ToolCallOutput> {
    match input.detail.as_deref() {
        None | Some("original") => {}
        Some(other) => anyhow::bail!("view_image.detail only supports `original`; got `{other}`"),
    }

    let target = state.target(&input.target)?;
    let response = target
        .client
        .image_read(&ImageReadRequest {
            path: input.path,
            workdir: input.workdir,
            detail: input.detail.clone(),
        })
        .await?;

    let mut image = serde_json::json!({
        "type": "input_image",
        "image_url": response.image_url,
    });
    if let Some(detail) = &response.detail {
        image["detail"] = serde_json::Value::String(detail.clone());
    }

    Ok(ToolCallOutput::content_and_structured(
        vec![Content::from_json(image)?],
        serde_json::to_value(ViewImageResult {
            target: input.target,
            image_url: response.image_url,
            detail: response.detail,
        })?,
    ))
}
```

```rust
// crates/remote-exec-broker/src/daemon_client.rs
impl DaemonClient {
    pub async fn patch_apply(&self, req: &PatchApplyRequest) -> anyhow::Result<PatchApplyResponse> {
        self.post("/v1/patch/apply", req).await
    }

    pub async fn image_read(&self, req: &ImageReadRequest) -> anyhow::Result<ImageReadResponse> {
        self.post("/v1/image/read", req).await
    }
}
```

```rust
// crates/remote-exec-broker/src/tools/mod.rs
pub mod exec;
pub mod image;
pub mod patch;
```

```rust
// crates/remote-exec-broker/src/mcp_server.rs
register_json_tool("exec_command", remote_exec_proto::public::ExecCommandInput::json_schema(), tools::exec::exec_command);
register_json_tool("write_stdin", remote_exec_proto::public::WriteStdinInput::json_schema(), tools::exec::write_stdin);
register_json_tool("apply_patch", remote_exec_proto::public::ApplyPatchInput::json_schema(), tools::patch::apply_patch);
register_json_tool("view_image", remote_exec_proto::public::ViewImageInput::json_schema(), tools::image::view_image);
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker mcp_assets -- --nocapture`
Expected: PASS with text-plus-structured patch output and `input_image` view-image output

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker
git commit -m "feat: add broker patch and image tools"
```

### Task 8: Add Multi-Target End-To-End Coverage And Developer Docs

**Files:**
- Create: `tests/e2e/multi_target.rs`
- Create: `configs/broker.example.toml`
- Create: `configs/daemon.example.toml`
- Modify: `README.md`
- Test/Verify: `cargo test --workspace && cargo fmt --all --check`

**Testing approach:** `characterization/integration test`
Reason: this task proves the actual multi-target system behavior described in the spec and finishes the repo with runnable docs.

- [ ] **Step 1: Write the failing whole-system test and example configs**

```rust
// tests/e2e/multi_target.rs
#[tokio::test]
async fn sessions_are_isolated_per_target() {
    let cluster = test_support::spawn_cluster().await;

    let started = cluster
        .broker
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "printf hello; sleep 2",
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    let session_id = started.structured_content["session_id"].as_str().unwrap();
    let polled = cluster
        .broker
        .call_tool(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-a",
                "chars": "",
                "yield_time_ms": 5000
            }),
        )
        .await;
    assert_eq!(polled.structured_content["target"], "builder-a");

    let mismatch = cluster
        .broker
        .call_tool_error(
            "write_stdin",
            serde_json::json!({
                "session_id": session_id,
                "target": "builder-b",
                "chars": ""
            }),
        )
        .await;
    assert!(mismatch.contains("does not belong"));
}
```

```toml
# configs/broker.example.toml
[targets.builder-a]
base_url = "https://builder-a.example.com:9443"
ca_pem = "/etc/remote-exec/ca.pem"
client_cert_pem = "/etc/remote-exec/broker.pem"
client_key_pem = "/etc/remote-exec/broker.key"
expected_daemon_name = "builder-a"

[targets.builder-b]
base_url = "https://builder-b.example.com:9443"
ca_pem = "/etc/remote-exec/ca.pem"
client_cert_pem = "/etc/remote-exec/broker.pem"
client_key_pem = "/etc/remote-exec/broker.key"
expected_daemon_name = "builder-b"
```

```toml
# configs/daemon.example.toml
target = "builder-a"
listen = "0.0.0.0:9443"
default_workdir = "/srv/work"

[tls]
cert_pem = "/etc/remote-exec/daemon.pem"
key_pem = "/etc/remote-exec/daemon.key"
ca_pem = "/etc/remote-exec/ca.pem"
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test --test multi_target sessions_are_isolated_per_target -- --exact`
Expected: FAIL because the cluster harness and multi-target broker wiring do not exist yet

- [ ] **Step 3: Implement the end-to-end harness, restart invalidation checks, and README runbook**

```markdown
# README.md
# remote-exec-mcp

Remote-first MCP server for running Codex-style local-system tools on multiple Linux machines.

## Components

- `remote-exec-broker`: public MCP server over stdio
- `remote-exec-daemon`: per-machine daemon over mTLS JSON/HTTP
- `remote-exec-proto`: shared protocol and public tool schemas

## Supported tools

- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

## Local development

```bash
cargo test --workspace
cargo fmt --all
```

## Trust model

Selecting a target is equivalent to `danger-full-access` on that machine. There is no per-call approval flow in v1.
```

```rust
// tests/e2e/multi_target.rs
mod test_support {
    pub struct ClusterFixture {
        pub broker: crate::BrokerFixture,
        pub daemon_a: crate::DaemonFixture,
        pub daemon_b: crate::DaemonFixture,
    }

    pub async fn spawn_cluster() -> ClusterFixture {
        let daemon_a = crate::DaemonFixture::spawn("builder-a").await;
        let daemon_b = crate::DaemonFixture::spawn("builder-b").await;
        let broker = crate::BrokerFixture::spawn(vec![
            ("builder-a".to_string(), daemon_a.config_fragment()),
            ("builder-b".to_string(), daemon_b.config_fragment()),
        ])
        .await;
        ClusterFixture { broker, daemon_a, daemon_b }
    }
}

#[tokio::test]
async fn patch_and_image_calls_only_touch_the_selected_target() {
    let cluster = test_support::spawn_cluster().await;

    cluster
        .broker
        .call_tool(
            "apply_patch",
            serde_json::json!({
                "target": "builder-a",
                "input": "*** Begin Patch\n*** Add File: marker.txt\n+builder-a\n*** End Patch\n"
            }),
        )
        .await;

    assert!(cluster.daemon_a.workdir.join("marker.txt").exists());
    assert!(!cluster.daemon_b.workdir.join("marker.txt").exists());
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test --workspace && cargo fmt --all --check`
Expected: PASS with the full workspace test suite green and no formatting drift

- [ ] **Step 5: Commit**

```bash
git add README.md configs tests/e2e
git commit -m "test: add multi-target end-to-end coverage"
```
