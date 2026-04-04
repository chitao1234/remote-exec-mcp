# Transfer Files Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `transfer_files` as one public broker tool for exact-path file and directory transfers between the broker host and configured remote targets.

**Architecture:** Keep the public API as one MCP tool. The broker validates public arguments, reserves `target: "local"` for broker-host filesystem access, exports the source endpoint into a temporary tar archive on the broker, and imports that archive at the destination endpoint. Remote endpoints gain dedicated daemon export/import HTTP routes, while broker-local endpoints use matching local helper code with the same private archive format and validation rules.

**Tech Stack:** Rust 2024, Tokio, axum, reqwest with streaming enabled, rmcp, serde/schemars, tempfile, tar, tokio-util, futures-util, cargo test

---

## File Map

- `Cargo.toml`
  - Add shared workspace dependencies for archive/stream handling: `futures-util`, `http-body-util`, `tar`, `tokio-util`.
  - Enable the `reqwest` `stream` feature workspace-wide for raw-body export/import.
- `crates/remote-exec-proto/src/public.rs`
  - Public `transfer_files` input/result structs and enums.
- `crates/remote-exec-proto/src/rpc.rs`
  - Broker-daemon transfer request/response structs, enums, and header-name constants used by the raw-body transfer routes.
- `crates/remote-exec-broker/Cargo.toml`
  - Add runtime dependencies needed by broker-local archive handling and request/response streaming.
- `crates/remote-exec-broker/src/config.rs`
  - Reserve the configured target name `local`.
- `crates/remote-exec-broker/src/lib.rs`
  - Re-run broker config validation before building state.
- `crates/remote-exec-broker/src/daemon_client.rs`
  - Add transfer export/import helpers that stream archive bytes between the broker temp file and daemon HTTP endpoints.
- `crates/remote-exec-broker/src/local_transfer.rs`
  - Broker-host archive export/import helpers for `target: "local"`.
- `crates/remote-exec-broker/src/mcp_server.rs`
  - Register the `transfer_files` tool.
- `crates/remote-exec-broker/src/tools/mod.rs`
  - Export the new transfer tool module.
- `crates/remote-exec-broker/src/tools/transfer.rs`
  - Public tool handler, endpoint validation, relay orchestration, and text rendering.
- `crates/remote-exec-broker/tests/mcp_transfer.rs`
  - Public broker-surface tests for tool registration plus `local -> local` behavior.
- `crates/remote-exec-broker/tests/support/mod.rs`
  - Reuse the existing broker fixture; no daemon transfer stubbing is needed for `local -> local` coverage.
- `crates/remote-exec-daemon/Cargo.toml`
  - Add runtime dependencies needed by daemon export/import handlers.
- `crates/remote-exec-daemon/src/lib.rs`
  - Export the new transfer module.
- `crates/remote-exec-daemon/src/server.rs`
  - Route `/v1/transfer/export` and `/v1/transfer/import`.
- `crates/remote-exec-daemon/src/transfer/mod.rs`
  - Export/import route handlers, HTTP header parsing, and daemon RPC error normalization.
- `crates/remote-exec-daemon/src/transfer/archive.rs`
  - Tar archive build/extract logic, filesystem validation, overwrite behavior, and summary counting.
- `crates/remote-exec-daemon/tests/support/mod.rs`
  - Add helpers for raw HTTP responses and raw-byte uploads.
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
  - Export/import RPC coverage including symlink rejection, exact destination semantics, and exec-bit preservation.
- `tests/e2e/multi_target.rs`
  - Real broker-plus-daemon tests for `local -> remote`, `remote -> local`, and `remote -> remote`.
- `README.md`
  - Document the new tool, the reserved `local` endpoint, focused test commands, and the broker-host trust-model expansion.

### Task 1: Add The Public `transfer_files` Types And Reserve `local`

**Files:**
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `crates/remote-exec-broker/src/lib.rs`
- Test/Verify: `cargo test -p remote-exec-broker load_rejects_reserved_local_target_name -- --nocapture`

**Testing approach:** `TDD`
Reason: the reserved-name rule has a clear unit-test seam, and the public types can land alongside that validation without starting the broker tool implementation yet.

- [ ] **Step 1: Add failing broker config tests for the reserved `local` target name**

```rust
// crates/remote-exec-broker/src/config.rs

#[cfg(test)]
mod tests {
    use super::BrokerConfig;

    #[tokio::test]
    async fn load_rejects_reserved_local_target_name() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.local]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("configured target name `local` is reserved"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn load_accepts_non_reserved_target_names() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-a]
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/broker.pem"
client_key_pem = "/tmp/broker.key"
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        assert!(config.targets.contains_key("builder-a"));
    }
}
```

- [ ] **Step 2: Run the focused verification and confirm the validation is missing**

Run: `cargo test -p remote-exec-broker load_rejects_reserved_local_target_name -- --nocapture`
Expected: FAIL because `BrokerConfig::load` currently accepts a configured target named `local`.

- [ ] **Step 3: Add the public tool structs and the reserved-name validation**

```rust
// crates/remote-exec-proto/src/public.rs

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwrite {
    Fail,
    Replace,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferEndpoint {
    pub target: String,
    pub path: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferFilesInput {
    pub source: TransferEndpoint,
    pub destination: TransferEndpoint,
    pub overwrite: TransferOverwrite,
    pub create_parent: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TransferFilesResult {
    pub source: TransferEndpoint,
    pub destination: TransferEndpoint,
    pub source_type: TransferSourceType,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub directories_copied: u64,
    pub replaced: bool,
}

// crates/remote-exec-broker/src/config.rs

impl BrokerConfig {
    fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.targets.contains_key("local"),
            "configured target name `local` is reserved for broker-host filesystem access"
        );
        Ok(())
    }

    pub async fn load(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let text = tokio::fs::read_to_string(path.as_ref())
            .await
            .with_context(|| format!("reading {}", path.as_ref().display()))?;
        let config: Self = toml::from_str(&text)?;
        config.validate()?;
        Ok(config)
    }
}

// crates/remote-exec-broker/src/lib.rs

async fn build_state(config: config::BrokerConfig) -> anyhow::Result<BrokerState> {
    config.validate()?;
    let mut targets = BTreeMap::new();
    // existing startup logic...
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker load_rejects_reserved_local_target_name -- --nocapture`
Expected: PASS, with the error text mentioning the reserved `local` target name.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/public.rs \
  crates/remote-exec-broker/src/config.rs \
  crates/remote-exec-broker/src/lib.rs
git commit -m "feat: reserve local transfer endpoint"
```

### Task 2: Add Daemon Export RPCs And Source Validation

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-daemon/Cargo.toml`
- Modify: `crates/remote-exec-daemon/src/lib.rs`
- Modify: `crates/remote-exec-daemon/src/server.rs`
- Create: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Create: `crates/remote-exec-daemon/src/transfer/archive.rs`
- Modify: `crates/remote-exec-daemon/tests/support/mod.rs`
- Create: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc export_ -- --nocapture`

**Testing approach:** `TDD`
Reason: daemon export is a clean, isolated RPC seam. It can be driven directly before any broker orchestration exists.

- [ ] **Step 1: Add export-focused daemon tests and the raw-response test helpers**

```rust
// crates/remote-exec-daemon/tests/support/mod.rs

impl DaemonFixture {
    pub async fn raw_post_json<Req>(&self, path: &str, body: &Req) -> reqwest::Response
    where
        Req: Serialize + ?Sized,
    {
        self.client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }
}

// crates/remote-exec-daemon/tests/transfer_rpc.rs

mod support;

use std::os::unix::fs::PermissionsExt;

use remote_exec_proto::rpc::{
    TransferExportRequest, TRANSFER_SOURCE_TYPE_HEADER,
};

#[tokio::test]
async fn export_file_streams_archive_and_reports_file_source_type() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("hello.txt");
    tokio::fs::write(&source, "hello\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(
        response
            .headers()
            .get(TRANSFER_SOURCE_TYPE_HEADER)
            .unwrap()
            .to_str()
            .unwrap(),
        "file"
    );
    assert!(!response.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn export_directory_rejects_nested_symlinks_before_streaming() {
    let fixture = support::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(&root).await.unwrap();
    tokio::fs::write(root.join("app.txt"), "ok\n").await.unwrap();
    std::os::unix::fs::symlink(root.join("app.txt"), root.join("app-link")).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(body.code, "transfer_source_unsupported");
}

#[tokio::test]
async fn export_rejects_symlink_source_root() {
    let fixture = support::spawn_daemon("builder-a").await;
    let target = fixture.workdir.join("target.txt");
    let link = fixture.workdir.join("root-link");
    tokio::fs::write(&target, "ok\n").await.unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: link.display().to_string(),
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(body.code, "transfer_source_unsupported");
}

#[tokio::test]
async fn export_file_preserves_executable_mode_in_archive_header() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("tool.sh");
    tokio::fs::write(&source, "#!/bin/sh\necho hi\n").await.unwrap();
    let mut perms = std::fs::metadata(&source).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&source, perms).unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = response.bytes().await.unwrap();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let mut entries = archive.entries().unwrap();
    let header = entries.next().unwrap().unwrap().header().clone();

    assert_eq!(header.mode().unwrap() & 0o111, 0o111);
}
```

- [ ] **Step 2: Run the focused verification and confirm the route is missing**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc export_ -- --nocapture`
Expected: FAIL because the transfer routes, shared RPC types, and archive dependencies do not exist yet.

- [ ] **Step 3: Add shared transfer RPC metadata and the export route scaffolding**

```rust
// Cargo.toml

[workspace.dependencies]
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
futures-util = "0.3"
http-body-util = "0.1"
tar = "0.4"
tokio-util = { version = "0.7", features = ["io"] }

// crates/remote-exec-daemon/Cargo.toml

[dependencies]
futures-util = { workspace = true }
http-body-util = { workspace = true }
tar = { workspace = true }
tempfile = { workspace = true }
tokio-util = { workspace = true }

// crates/remote-exec-proto/src/rpc.rs

pub const TRANSFER_SOURCE_TYPE_HEADER: &str = "x-remote-exec-source-type";
pub const TRANSFER_DESTINATION_PATH_HEADER: &str = "x-remote-exec-destination-path";
pub const TRANSFER_OVERWRITE_HEADER: &str = "x-remote-exec-overwrite";
pub const TRANSFER_CREATE_PARENT_HEADER: &str = "x-remote-exec-create-parent";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferSourceType {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferOverwriteMode {
    Fail,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferExportRequest {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportRequest {
    pub destination_path: String,
    pub overwrite: TransferOverwriteMode,
    pub create_parent: bool,
    pub source_type: TransferSourceType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferImportResponse {
    pub source_type: TransferSourceType,
    pub bytes_copied: u64,
    pub files_copied: u64,
    pub directories_copied: u64,
    pub replaced: bool,
}

// crates/remote-exec-daemon/src/lib.rs
pub mod transfer;

// crates/remote-exec-daemon/src/server.rs
.route("/v1/transfer/export", post(crate::transfer::export_path))
.route("/v1/transfer/import", post(crate::transfer::import_archive))
```

- [ ] **Step 4: Implement export path validation and archive streaming**

```rust
// crates/remote-exec-daemon/src/transfer/archive.rs

use std::os::unix::fs::PermissionsExt;

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

pub struct ExportedArchive {
    pub source_type: remote_exec_proto::rpc::TransferSourceType,
    pub temp_path: tempfile::TempPath,
}

pub async fn export_path_to_archive(path: &std::path::Path) -> anyhow::Result<ExportedArchive> {
    anyhow::ensure!(path.is_absolute(), "transfer source path `{}` is not absolute", path.display());

    let metadata = tokio::fs::symlink_metadata(path).await?;
    let source_type = if metadata.file_type().is_file() {
        remote_exec_proto::rpc::TransferSourceType::File
    } else if metadata.file_type().is_dir() {
        remote_exec_proto::rpc::TransferSourceType::Directory
    } else {
        anyhow::bail!("transfer source path `{}` is not a regular file or directory", path.display());
    };

    let temp = tempfile::NamedTempFile::new()?;
    let temp_path = temp.into_temp_path();
    let archive_path = temp_path.to_path_buf();
    let source_path = path.to_path_buf();
    let source_type_for_task = source_type.clone();

    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&archive_path)?;
        let mut builder = tar::Builder::new(file);

        match source_type_for_task {
            remote_exec_proto::rpc::TransferSourceType::File => {
                builder.append_path_with_name(&source_path, SINGLE_FILE_ENTRY)?;
            }
            remote_exec_proto::rpc::TransferSourceType::Directory => {
                append_directory_tree(&mut builder, &source_path)?;
            }
        }

        builder.finish()?;
        Ok(())
    })
    .await??;

    Ok(ExportedArchive {
        source_type,
        temp_path,
    })
}

fn append_directory_tree(
    builder: &mut tar::Builder<std::fs::File>,
    root: &std::path::Path,
) -> anyhow::Result<()> {
    builder.append_dir(".", root)?;
    append_directory_entries(builder, root, root)
}

fn append_directory_entries(
    builder: &mut tar::Builder<std::fs::File>,
    root: &std::path::Path,
    current: &std::path::Path,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "transfer source contains unsupported symlink `{}`",
            path.display()
        );
        if metadata.is_dir() {
            builder.append_dir(rel, &path)?;
            append_directory_entries(builder, root, &path)?;
        } else if metadata.is_file() {
            builder.append_path_with_name(&path, rel)?;
        } else {
            anyhow::bail!("transfer source contains unsupported entry `{}`", path.display());
        }
    }
    Ok(())
}

// crates/remote-exec-daemon/src/transfer/mod.rs

use futures_util::TryStreamExt;
use http_body_util::BodyExt;

pub async fn export_path(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<TransferExportRequest>,
) -> Result<axum::response::Response, (StatusCode, Json<RpcErrorBody>)> {
    let exported = archive::export_path_to_archive(std::path::Path::new(&req.path))
        .await
        .map_err(map_transfer_error)?;

    let file = tokio::fs::File::open(exported.temp_path.to_path_buf())
        .await
        .map_err(crate::exec::internal_error)?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);

    Ok((
        [
            (
                remote_exec_proto::rpc::TRANSFER_SOURCE_TYPE_HEADER,
                serde_json::to_string(&exported.source_type)
                    .unwrap()
                    .trim_matches('"')
                    .to_string(),
            ),
        ],
        body,
    )
        .into_response())
}

fn map_transfer_error(err: anyhow::Error) -> (StatusCode, Json<RpcErrorBody>) {
    let message = err.to_string();
    let code = if message.contains("not absolute") {
        "transfer_path_not_absolute"
    } else if message.contains("already exists") {
        "transfer_destination_exists"
    } else if message.contains("parent") && message.contains("does not exist") {
        "transfer_parent_missing"
    } else if message.contains("unsupported symlink")
        || message.contains("unsupported entry")
        || message.contains("regular file or directory")
    {
        "transfer_source_unsupported"
    } else if message.contains("No such file or directory") {
        "transfer_source_missing"
    } else {
        "transfer_failed"
    };
    crate::exec::rpc_error(code, message)
}
```

- [ ] **Step 5: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc export_ -- --nocapture`
Expected: PASS with successful file export, symlink rejection, and executable mode preserved in the tar header.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml \
  crates/remote-exec-proto/src/rpc.rs \
  crates/remote-exec-daemon/Cargo.toml \
  crates/remote-exec-daemon/src/lib.rs \
  crates/remote-exec-daemon/src/server.rs \
  crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/transfer/archive.rs \
  crates/remote-exec-daemon/tests/support/mod.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs
git commit -m "feat: add daemon transfer export"
```

### Task 3: Add Daemon Import RPCs, Replace Semantics, And Transfer Summaries

**Files:**
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/archive.rs`
- Modify: `crates/remote-exec-daemon/tests/support/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc import_ -- --nocapture`

**Testing approach:** `TDD`
Reason: import owns the exact-path semantics, overwrite behavior, and summary counters. That behavior is easiest to pin down at the daemon RPC layer before the broker relay is involved.

- [ ] **Step 1: Add failing import-focused daemon RPC tests**

```rust
// crates/remote-exec-daemon/tests/support/mod.rs

impl DaemonFixture {
    pub async fn raw_post_bytes(
        &self,
        path: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> reqwest::Response {
        let mut request = self.client.post(self.url(path));
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        request.body(body).send().await.unwrap()
    }
}

// crates/remote-exec-daemon/tests/transfer_rpc.rs

use remote_exec_proto::rpc::{
    TransferImportResponse, TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER,
    TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
};

#[tokio::test]
async fn import_directory_replaces_exact_destination_and_preserves_exec_bits() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source_root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(source_root.join("empty")).await.unwrap();
    tokio::fs::create_dir_all(source_root.join("bin")).await.unwrap();
    tokio::fs::write(source_root.join("bin/tool.sh"), "#!/bin/sh\necho hi\n")
        .await
        .unwrap();
    let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &remote_exec_proto::rpc::TransferExportRequest {
                path: source_root.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();
    let destination = fixture.workdir.join("release");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "true".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "directory".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    let summary = response.json::<TransferImportResponse>().await.unwrap();
    assert_eq!(summary.source_type, remote_exec_proto::rpc::TransferSourceType::Directory);
    assert_eq!(summary.files_copied, 1);
    assert_eq!(summary.directories_copied, 3);
    assert!(!summary.replaced);
    assert!(destination.join("empty").is_dir());
    assert_eq!(
        std::fs::metadata(destination.join("bin/tool.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
}

#[tokio::test]
async fn import_rejects_existing_destination_when_overwrite_is_fail() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
    let destination = fixture.workdir.join("dest.txt");
    tokio::fs::write(&source, "new\n").await.unwrap();
    tokio::fs::write(&destination, "old\n").await.unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &remote_exec_proto::rpc::TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_destination_exists");
    assert_eq!(tokio::fs::read_to_string(&destination).await.unwrap(), "old\n");
}

#[tokio::test]
async fn import_replaces_directory_with_file_at_the_exact_destination_path() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("tool.txt");
    let destination = fixture.workdir.join("release");
    tokio::fs::write(&source, "artifact\n").await.unwrap();
    tokio::fs::create_dir_all(destination.join("nested")).await.unwrap();
    tokio::fs::write(destination.join("nested/old.txt"), "old\n")
        .await
        .unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &remote_exec_proto::rpc::TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "replace".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert!(response.status().is_success());
    assert_eq!(tokio::fs::read_to_string(&destination).await.unwrap(), "artifact\n");
}

#[tokio::test]
async fn import_rejects_missing_parent_when_create_parent_is_false() {
    let fixture = support::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("source.txt");
    let destination = fixture.workdir.join("missing/child.txt");
    tokio::fs::write(&source, "artifact\n").await.unwrap();

    let exported = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &remote_exec_proto::rpc::TransferExportRequest {
                path: source.display().to_string(),
            },
        )
        .await;
    let bytes = exported.bytes().await.unwrap().to_vec();

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &[
                (
                    TRANSFER_DESTINATION_PATH_HEADER,
                    destination.display().to_string(),
                ),
                (TRANSFER_OVERWRITE_HEADER, "fail".to_string()),
                (TRANSFER_CREATE_PARENT_HEADER, "false".to_string()),
                (TRANSFER_SOURCE_TYPE_HEADER, "file".to_string()),
            ],
            bytes,
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_parent_missing");
    assert!(!destination.exists());
}
```

- [ ] **Step 2: Run the focused verification and confirm import is not implemented**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc import_ -- --nocapture`
Expected: FAIL because `/v1/transfer/import` does not yet parse headers, receive raw archive bytes, or materialize anything.

- [ ] **Step 3: Implement archive extraction, overwrite handling, and summary counting**

```rust
// crates/remote-exec-daemon/src/transfer/archive.rs

use std::os::unix::fs::PermissionsExt;

pub async fn import_archive_from_file(
    archive_path: &std::path::Path,
    request: &remote_exec_proto::rpc::TransferImportRequest,
) -> anyhow::Result<remote_exec_proto::rpc::TransferImportResponse> {
    let destination = std::path::Path::new(&request.destination_path);
    anyhow::ensure!(
        destination.is_absolute(),
        "transfer destination path `{}` is not absolute",
        destination.display()
    );

    let replaced = prepare_destination(destination, request).await?;
    let archive_path = archive_path.to_path_buf();
    let destination_path = destination.to_path_buf();
    let request = request.clone();

    tokio::task::spawn_blocking(move || extract_archive(&archive_path, &destination_path, &request, replaced))
        .await?
}

async fn prepare_destination(
    destination: &std::path::Path,
    request: &remote_exec_proto::rpc::TransferImportRequest,
) -> anyhow::Result<bool> {
    if let Some(parent) = destination.parent() {
        if request.create_parent {
            tokio::fs::create_dir_all(parent).await?;
        } else {
            anyhow::ensure!(
                tokio::fs::metadata(parent).await.map(|m| m.is_dir()).unwrap_or(false),
                "destination parent `{}` does not exist",
                parent.display()
            );
        }
    }

    match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) => match request.overwrite {
            remote_exec_proto::rpc::TransferOverwriteMode::Fail => {
                anyhow::bail!("destination path `{}` already exists", destination.display());
            }
            remote_exec_proto::rpc::TransferOverwriteMode::Replace => {
                if metadata.is_dir() {
                    tokio::fs::remove_dir_all(destination).await?;
                } else {
                    tokio::fs::remove_file(destination).await?;
                }
                Ok(true)
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn extract_archive(
    archive_path: &std::path::Path,
    destination_path: &std::path::Path,
    request: &remote_exec_proto::rpc::TransferImportRequest,
    replaced: bool,
) -> anyhow::Result<remote_exec_proto::rpc::TransferImportResponse> {
    let mut summary = remote_exec_proto::rpc::TransferImportResponse {
        source_type: request.source_type.clone(),
        bytes_copied: 0,
        files_copied: 0,
        directories_copied: matches!(
            request.source_type,
            remote_exec_proto::rpc::TransferSourceType::Directory
        ) as u64,
        replaced,
    };

    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);

    match request.source_type {
        remote_exec_proto::rpc::TransferSourceType::File => {
            let mut entries = archive.entries()?;
            let mut entry = entries.next().ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
            anyhow::ensure!(entry.header().entry_type().is_file(), "archive entry is not a regular file");
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;
            if let Some(parent) = destination_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(destination_path, &bytes)?;
            if entry.header().mode()? & 0o111 != 0 {
                let mut perms = std::fs::metadata(destination_path)?.permissions();
                std::os::unix::fs::PermissionsExt::set_mode(&mut perms, perms.mode() | 0o111);
                std::fs::set_permissions(destination_path, perms)?;
            }
            summary.bytes_copied = bytes.len() as u64;
            summary.files_copied = 1;
        }
        remote_exec_proto::rpc::TransferSourceType::Directory => {
            std::fs::create_dir_all(destination_path)?;
            for entry in archive.entries()? {
                let mut entry = entry?;
                let rel = entry.path()?.to_path_buf();
                if rel == std::path::Path::new(".") {
                    continue;
                }
                let out = destination_path.join(&rel);
                let entry_type = entry.header().entry_type();
                anyhow::ensure!(
                    entry_type.is_dir() || entry_type.is_file(),
                    "archive contains unsupported entry `{}`",
                    rel.display()
                );
                if entry_type.is_dir() {
                    std::fs::create_dir_all(&out)?;
                    summary.directories_copied += 1;
                    continue;
                }
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                std::fs::write(&out, &bytes)?;
                if entry.header().mode()? & 0o111 != 0 {
                    let mut perms = std::fs::metadata(&out)?.permissions();
                    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, perms.mode() | 0o111);
                    std::fs::set_permissions(&out, perms)?;
                }
                summary.bytes_copied += bytes.len() as u64;
                summary.files_copied += 1;
            }
        }
    }

    Ok(summary)
}

// crates/remote-exec-daemon/src/transfer/mod.rs

pub async fn import_archive(
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Result<Json<TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)> {
    let request = parse_import_request(&headers)?;
    let temp = tempfile::NamedTempFile::new().map_err(crate::exec::internal_error)?;
    let temp_path = temp.into_temp_path();
    let mut file = tokio::fs::File::create(temp_path.to_path_buf())
        .await
        .map_err(crate::exec::internal_error)?;
    let mut stream = tokio_util::io::StreamReader::new(
        BodyExt::into_data_stream(body)
            .map_err(std::io::Error::other),
    );
    tokio::io::copy(&mut stream, &mut file)
        .await
        .map_err(crate::exec::internal_error)?;

    let summary = archive::import_archive_from_file(&temp_path, &request)
        .await
        .map_err(map_transfer_error)?;
    Ok(Json(summary))
}

fn parse_import_request(
    headers: &axum::http::HeaderMap,
) -> Result<TransferImportRequest, (StatusCode, Json<RpcErrorBody>)> {
    Ok(TransferImportRequest {
        destination_path: header_string(
            headers,
            remote_exec_proto::rpc::TRANSFER_DESTINATION_PATH_HEADER,
        )?,
        overwrite: parse_header_enum(
            headers,
            remote_exec_proto::rpc::TRANSFER_OVERWRITE_HEADER,
        )?,
        create_parent: header_string(
            headers,
            remote_exec_proto::rpc::TRANSFER_CREATE_PARENT_HEADER,
        )?
        .parse::<bool>()
        .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))?,
        source_type: parse_header_enum(
            headers,
            remote_exec_proto::rpc::TRANSFER_SOURCE_TYPE_HEADER,
        )?,
    })
}

fn header_string(
    headers: &axum::http::HeaderMap,
    name: &str,
) -> Result<String, (StatusCode, Json<RpcErrorBody>)> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| crate::exec::rpc_error("transfer_failed", format!("missing header `{name}`")))
}

fn parse_header_enum<T>(
    headers: &axum::http::HeaderMap,
    name: &str,
) -> Result<T, (StatusCode, Json<RpcErrorBody>)>
where
    T: serde::de::DeserializeOwned,
{
    let raw = header_string(headers, name)?;
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| crate::exec::rpc_error("transfer_failed", err.to_string()))
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc import_ -- --nocapture`
Expected: PASS with exact-path replacement, `overwrite: fail` rejection before mutation, directory root counting, and executable-bit preservation.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/transfer/archive.rs \
  crates/remote-exec-daemon/tests/support/mod.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs
git commit -m "feat: add daemon transfer import"
```

### Task 4: Expose `transfer_files` In The Broker And Cover `local -> local`

**Files:**
- Modify: `crates/remote-exec-broker/Cargo.toml`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Create: `crates/remote-exec-broker/src/local_transfer.rs`
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Modify: `crates/remote-exec-broker/src/tools/mod.rs`
- Create: `crates/remote-exec-broker/src/tools/transfer.rs`
- Create: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`

**Testing approach:** `TDD`
Reason: the broker tool contract is the real user-facing seam. `local -> local` covers endpoint validation, exact destination semantics, and result formatting without depending on remote daemon behavior.

- [ ] **Step 1: Add failing public broker tests for tool registration and `local -> local` transfers**

```rust
// crates/remote-exec-broker/tests/mcp_transfer.rs

mod support;

use rmcp::model::PaginatedRequestParams;

#[tokio::test]
async fn transfer_files_is_listed_for_mcp_clients() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let tools = fixture
        .client
        .list_tools(Some(PaginatedRequestParams {
            meta: None,
            cursor: None,
        }))
        .await
        .expect("list tools");

    assert!(tools
        .tools
        .iter()
        .any(|tool| tool.name.as_ref() == "transfer_files"));
}

#[tokio::test]
async fn transfer_files_copies_local_file_and_reports_summary() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let source = fixture._tempdir.path().join("source.txt");
    let destination = fixture._tempdir.path().join("dest.txt");
    std::fs::write(&source, "hello\n").unwrap();

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": false
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "hello\n");
    assert_eq!(result.structured_content["source_type"], "file");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert_eq!(result.structured_content["directories_copied"], 0);
    assert_eq!(result.structured_content["bytes_copied"], 6);
    assert_eq!(result.structured_content["replaced"], false);
}

#[tokio::test]
async fn transfer_files_rejects_same_local_path_before_mutation() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let source = fixture._tempdir.path().join("same.txt");
    std::fs::write(&source, "hello\n").unwrap();

    let error = fixture
        .call_tool_error(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": false
            }),
        )
        .await;

    assert!(error.contains("source and destination must differ"));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "hello\n");
}
```

- [ ] **Step 2: Run the focused verification and confirm the public tool is missing**

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: FAIL because the broker does not register or implement `transfer_files`.

- [ ] **Step 3: Add broker-local archive helpers and daemon client transfer methods**

```rust
// crates/remote-exec-broker/Cargo.toml

[dependencies]
futures-util = { workspace = true }
http-body-util = { workspace = true }
tar = { workspace = true }
tempfile = { workspace = true }
tokio-util = { workspace = true }

// crates/remote-exec-broker/src/local_transfer.rs

use std::os::unix::fs::PermissionsExt;

use remote_exec_proto::rpc::{
    TransferImportRequest, TransferImportResponse, TransferOverwriteMode, TransferSourceType,
};

const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

pub async fn export_path_to_archive(
    path: &std::path::Path,
    archive_path: &std::path::Path,
) -> anyhow::Result<TransferSourceType> {
    anyhow::ensure!(path.is_absolute(), "transfer source path `{}` is not absolute", path.display());
    let metadata = tokio::fs::symlink_metadata(path).await?;
    let source_type = if metadata.file_type().is_file() {
        TransferSourceType::File
    } else if metadata.file_type().is_dir() {
        TransferSourceType::Directory
    } else {
        anyhow::bail!("transfer source path `{}` is not a regular file or directory", path.display());
    };

    let source = path.to_path_buf();
    let destination = archive_path.to_path_buf();
    let source_type_for_task = source_type.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let file = std::fs::File::create(&destination)?;
        let mut builder = tar::Builder::new(file);
        match source_type_for_task {
            TransferSourceType::File => builder.append_path_with_name(&source, SINGLE_FILE_ENTRY)?,
            TransferSourceType::Directory => {
                builder.append_dir(".", &source)?;
                append_directory_entries(&mut builder, &source, &source)?;
            }
        }
        builder.finish()?;
        Ok(())
    })
    .await??;
    Ok(source_type)
}

pub async fn import_archive_from_file(
    archive_path: &std::path::Path,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    let destination = std::path::PathBuf::from(&request.destination_path);
    let replaced = prepare_destination(&destination, request).await?;
    let archive = archive_path.to_path_buf();
    let request = request.clone();
    tokio::task::spawn_blocking(move || extract_archive(&archive, &destination, &request, replaced))
        .await?
}

async fn prepare_destination(
    destination: &std::path::Path,
    request: &TransferImportRequest,
) -> anyhow::Result<bool> {
    if let Some(parent) = destination.parent() {
        if request.create_parent {
            tokio::fs::create_dir_all(parent).await?;
        } else {
            anyhow::ensure!(
                tokio::fs::metadata(parent).await.map(|m| m.is_dir()).unwrap_or(false),
                "destination parent `{}` does not exist",
                parent.display()
            );
        }
    }

    match tokio::fs::symlink_metadata(destination).await {
        Ok(metadata) => match request.overwrite {
            remote_exec_proto::rpc::TransferOverwriteMode::Fail => {
                anyhow::bail!("destination path `{}` already exists", destination.display());
            }
            remote_exec_proto::rpc::TransferOverwriteMode::Replace => {
                if metadata.is_dir() {
                    tokio::fs::remove_dir_all(destination).await?;
                } else {
                    tokio::fs::remove_file(destination).await?;
                }
                Ok(true)
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

fn append_directory_entries(
    builder: &mut tar::Builder<std::fs::File>,
    root: &std::path::Path,
    current: &std::path::Path,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "transfer source contains unsupported symlink `{}`",
            path.display()
        );
        if metadata.is_dir() {
            builder.append_dir(rel, &path)?;
            append_directory_entries(builder, root, &path)?;
        } else if metadata.is_file() {
            builder.append_path_with_name(&path, rel)?;
        } else {
            anyhow::bail!("transfer source contains unsupported entry `{}`", path.display());
        }
    }
    Ok(())
}

fn extract_archive(
    archive_path: &std::path::Path,
    destination_path: &std::path::Path,
    request: &TransferImportRequest,
    replaced: bool,
) -> anyhow::Result<TransferImportResponse> {
    let mut summary = TransferImportResponse {
        source_type: request.source_type.clone(),
        bytes_copied: 0,
        files_copied: 0,
        directories_copied: matches!(request.source_type, TransferSourceType::Directory) as u64,
        replaced,
    };

    let file = std::fs::File::open(archive_path)?;
    let mut archive = tar::Archive::new(file);
    match request.source_type {
        TransferSourceType::File => {
            let mut entries = archive.entries()?;
            let mut entry = entries.next().ok_or_else(|| anyhow::anyhow!("archive is empty"))??;
            anyhow::ensure!(entry.header().entry_type().is_file(), "archive entry is not a regular file");
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut bytes)?;
            if let Some(parent) = destination_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(destination_path, &bytes)?;
            if entry.header().mode()? & 0o111 != 0 {
                let mut perms = std::fs::metadata(destination_path)?.permissions();
                std::os::unix::fs::PermissionsExt::set_mode(&mut perms, perms.mode() | 0o111);
                std::fs::set_permissions(destination_path, perms)?;
            }
            anyhow::ensure!(entries.next().transpose()?.is_none(), "file archive contains extra entries");
            summary.bytes_copied = bytes.len() as u64;
            summary.files_copied = 1;
        }
        TransferSourceType::Directory => {
            std::fs::create_dir_all(destination_path)?;
            for entry in archive.entries()? {
                let mut entry = entry?;
                let rel = entry.path()?.to_path_buf();
                if rel == std::path::Path::new(".") {
                    continue;
                }
                let out = destination_path.join(&rel);
                let entry_type = entry.header().entry_type();
                anyhow::ensure!(
                    entry_type.is_dir() || entry_type.is_file(),
                    "archive contains unsupported entry `{}`",
                    rel.display()
                );
                if entry_type.is_dir() {
                    std::fs::create_dir_all(&out)?;
                    summary.directories_copied += 1;
                } else {
                    if let Some(parent) = out.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut entry, &mut bytes)?;
                    std::fs::write(&out, &bytes)?;
                    if entry.header().mode()? & 0o111 != 0 {
                        let mut perms = std::fs::metadata(&out)?.permissions();
                        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, perms.mode() | 0o111);
                        std::fs::set_permissions(&out, perms)?;
                    }
                    summary.bytes_copied += bytes.len() as u64;
                    summary.files_copied += 1;
                }
            }
        }
    }

    Ok(summary)
}

// crates/remote-exec-broker/src/daemon_client.rs

use futures_util::TryStreamExt;

pub async fn transfer_export_to_file(
    &self,
    req: &remote_exec_proto::rpc::TransferExportRequest,
    archive_path: &std::path::Path,
) -> Result<remote_exec_proto::rpc::TransferSourceType, DaemonClientError> {
    let response = self
        .client
        .post(format!("{}{}", self.base_url, "/v1/transfer/export"))
        .header(CONNECTION, "close")
        .json(req)
        .send()
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    if !response.status().is_success() {
        return Err(decode_rpc_error(response).await);
    }
    let source_type = parse_header_enum(
        response.headers(),
        remote_exec_proto::rpc::TRANSFER_SOURCE_TYPE_HEADER,
    )?;
    let mut file = tokio::fs::File::create(archive_path)
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    let mut stream = tokio_util::io::StreamReader::new(
        response.bytes_stream().map_err(std::io::Error::other),
    );
    tokio::io::copy(&mut stream, &mut file)
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    Ok(source_type)
}

pub async fn transfer_import_from_file(
    &self,
    archive_path: &std::path::Path,
    req: &remote_exec_proto::rpc::TransferImportRequest,
) -> Result<remote_exec_proto::rpc::TransferImportResponse, DaemonClientError> {
    let file = tokio::fs::File::open(archive_path)
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    let response = self
        .client
        .post(format!("{}{}", self.base_url, "/v1/transfer/import"))
        .header(CONNECTION, "close")
        .header(
            remote_exec_proto::rpc::TRANSFER_DESTINATION_PATH_HEADER,
            req.destination_path.clone(),
        )
        .header(
            remote_exec_proto::rpc::TRANSFER_OVERWRITE_HEADER,
            serde_json::to_string(&req.overwrite).unwrap().trim_matches('"').to_string(),
        )
        .header(
            remote_exec_proto::rpc::TRANSFER_CREATE_PARENT_HEADER,
            req.create_parent.to_string(),
        )
        .header(
            remote_exec_proto::rpc::TRANSFER_SOURCE_TYPE_HEADER,
            serde_json::to_string(&req.source_type).unwrap().trim_matches('"').to_string(),
        )
        .body(body)
        .send()
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    if !response.status().is_success() {
        return Err(decode_rpc_error(response).await);
    }
    response
        .json()
        .await
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

fn parse_header_enum<T>(
    headers: &reqwest::header::HeaderMap,
    name: &str,
) -> Result<T, DaemonClientError>
where
    T: serde::de::DeserializeOwned,
{
    let raw = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| DaemonClientError::Decode(anyhow::anyhow!("missing header `{name}`")))?;
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

async fn decode_rpc_error(response: reqwest::Response) -> DaemonClientError {
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|err| err.to_string());
    if let Ok(error) = serde_json::from_str::<remote_exec_proto::rpc::RpcErrorBody>(&body) {
        DaemonClientError::Rpc {
            status,
            code: Some(error.code),
            message: error.message,
        }
    } else {
        DaemonClientError::Rpc {
            status,
            code: None,
            message: body,
        }
    }
}
```

- [ ] **Step 4: Register the tool and implement the public broker handler**

```rust
// crates/remote-exec-broker/src/tools/mod.rs
pub mod transfer;

// crates/remote-exec-broker/src/mcp_server.rs

#[tool(
    name = "transfer_files",
    description = "Transfer one file or one directory tree between broker-local and configured target filesystems."
)]
async fn transfer_files(
    &self,
    Parameters(input): Parameters<remote_exec_proto::public::TransferFilesInput>,
) -> Result<CallToolResult, McpError> {
    Ok(match crate::tools::transfer::transfer_files(&self.state, input).await {
        Ok(output) => output.into_call_tool_result(),
        Err(err) => format_tool_error(err),
    })
}

// crates/remote-exec-broker/src/tools/transfer.rs

use std::path::{Path, PathBuf};

use remote_exec_proto::public::{
    TransferEndpoint, TransferFilesInput, TransferFilesResult, TransferOverwrite,
    TransferSourceType,
};
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferOverwriteMode,
};

use crate::daemon_client::DaemonClientError;
use crate::mcp_server::ToolCallOutput;

pub async fn transfer_files(
    state: &crate::BrokerState,
    input: TransferFilesInput,
) -> anyhow::Result<ToolCallOutput> {
    ensure_absolute(&input.source)?;
    ensure_absolute(&input.destination)?;
    ensure_distinct_endpoints(&input.source, &input.destination)?;

    let temp = tempfile::NamedTempFile::new()?;
    let archive_path = temp.path().to_path_buf();
    let source_type = export_endpoint_to_archive(state, &input.source, &archive_path).await?;
    let import_summary = import_archive_to_endpoint(
        state,
        &archive_path,
        &input.destination,
        &input.overwrite,
        &source_type,
        input.create_parent,
    )
    .await?;
    let result = TransferFilesResult {
        source: input.source.clone(),
        destination: input.destination.clone(),
        source_type: match source_type {
            remote_exec_proto::rpc::TransferSourceType::File => TransferSourceType::File,
            remote_exec_proto::rpc::TransferSourceType::Directory => TransferSourceType::Directory,
        },
        bytes_copied: import_summary.bytes_copied,
        files_copied: import_summary.files_copied,
        directories_copied: import_summary.directories_copied,
        replaced: import_summary.replaced,
    };

    Ok(crate::mcp_server::ToolCallOutput::text_and_structured(
        format_transfer_text(&result),
        serde_json::to_value(result)?,
    ))
}

async fn export_endpoint_to_archive(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
    archive_path: &Path,
) -> anyhow::Result<remote_exec_proto::rpc::TransferSourceType> {
    match endpoint.target.as_str() {
        "local" => crate::local_transfer::export_path_to_archive(Path::new(&endpoint.path), archive_path).await,
        target_name => {
            let target = state.target(target_name)?;
            target.ensure_identity_verified(target_name).await?;
            match target
                .client
                .transfer_export_to_file(
                    &TransferExportRequest {
                        path: endpoint.path.clone(),
                    },
                    archive_path,
                )
                .await
            {
                Ok(source_type) => Ok(source_type),
                Err(err) => {
                    if matches!(err, DaemonClientError::Transport(_)) {
                        target.clear_cached_daemon_info().await;
                    }
                    Err(normalize_transfer_error(err))
                }
            }
        }
    }
}

async fn import_archive_to_endpoint(
    state: &crate::BrokerState,
    archive_path: &Path,
    endpoint: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    source_type: &remote_exec_proto::rpc::TransferSourceType,
    create_parent: bool,
) -> anyhow::Result<remote_exec_proto::rpc::TransferImportResponse> {
    let request = TransferImportRequest {
        destination_path: endpoint.path.clone(),
        overwrite: match overwrite {
            TransferOverwrite::Fail => TransferOverwriteMode::Fail,
            TransferOverwrite::Replace => TransferOverwriteMode::Replace,
        },
        create_parent,
        source_type: source_type.clone(),
    };

    match endpoint.target.as_str() {
        "local" => crate::local_transfer::import_archive_from_file(archive_path, &request).await,
        target_name => {
            let target = state.target(target_name)?;
            target.ensure_identity_verified(target_name).await?;
            match target
                .client
                .transfer_import_from_file(archive_path, &request)
                .await
            {
                Ok(summary) => Ok(summary),
                Err(err) => {
                    if matches!(err, DaemonClientError::Transport(_)) {
                        target.clear_cached_daemon_info().await;
                    }
                    Err(normalize_transfer_error(err))
                }
            }
        }
    }
}

fn ensure_absolute(endpoint: &TransferEndpoint) -> anyhow::Result<()> {
    anyhow::ensure!(
        Path::new(&endpoint.path).is_absolute(),
        "transfer endpoint path `{}` is not absolute",
        endpoint.path
    );
    Ok(())
}

fn ensure_distinct_endpoints(
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    let source_path = PathBuf::from(&source.path);
    let destination_path = PathBuf::from(&destination.path);
    anyhow::ensure!(
        !(source.target == destination.target && source_path == destination_path),
        "source and destination must differ"
    );
    Ok(())
}

fn normalize_transfer_error(err: DaemonClientError) -> anyhow::Error {
    match err {
        DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}

fn format_transfer_text(result: &TransferFilesResult) -> String {
    format!(
        "Transferred {} `{}` from `{}` to `{}` on `{}`.\nFiles: {}, directories: {}, bytes: {}, replaced: {}",
        match result.source_type {
            TransferSourceType::File => "file",
            TransferSourceType::Directory => "directory",
        },
        result.source.path,
        result.source.target,
        result.destination.path,
        result.destination.target,
        result.files_copied,
        result.directories_copied,
        result.bytes_copied,
        if result.replaced { "yes" } else { "no" }
    )
}
```

- [ ] **Step 5: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: PASS with tool registration, successful `local -> local` file transfer, exact summary counters, and same-path rejection before mutation.

- [ ] **Step 6: Commit**

```bash
git add crates/remote-exec-broker/Cargo.toml \
  crates/remote-exec-broker/src/daemon_client.rs \
  crates/remote-exec-broker/src/local_transfer.rs \
  crates/remote-exec-broker/src/mcp_server.rs \
  crates/remote-exec-broker/src/tools/mod.rs \
  crates/remote-exec-broker/src/tools/transfer.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "feat: add broker transfer_files tool"
```

### Task 5: Add Real Cross-Target Coverage, Update Docs, And Run The Quality Gate

**Files:**
- Modify: `tests/e2e/multi_target.rs`
- Modify: `README.md`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `characterization/integration test`
Reason: this task proves the full broker-plus-daemon contract and then aligns the user-facing docs with the finished behavior.

- [ ] **Step 1: Add end-to-end tests for the three important transfer directions**

```rust
// tests/e2e/multi_target.rs

use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn transfer_files_copies_local_file_to_remote_exact_destination_path() {
    let cluster = support::spawn_cluster().await;
    let local_dir = tempfile::tempdir().unwrap();
    let source = local_dir.path().join("artifact.txt");
    std::fs::write(&source, "artifact\n").unwrap();
    let destination = cluster.daemon_a.workdir.join("releases/current.txt");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "local",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "builder-a",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "artifact\n");
    assert_eq!(result.structured_content["destination"]["target"], "builder-a");
    assert!(!cluster.daemon_a.workdir.join("releases/artifact.txt").exists());
}

#[tokio::test]
async fn transfer_files_copies_remote_file_back_to_local() {
    let cluster = support::spawn_cluster().await;
    let source = cluster.daemon_a.workdir.join("build.log");
    std::fs::write(&source, "done\n").unwrap();
    let local_dir = tempfile::tempdir().unwrap();
    let destination = local_dir.path().join("logs/build.log");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source.display().to_string()
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "overwrite": "fail",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "done\n");
    assert_eq!(result.structured_content["source"]["target"], "builder-a");
}

#[tokio::test]
async fn transfer_files_moves_remote_directory_between_targets_without_basename_inference() {
    let cluster = support::spawn_cluster().await;
    let source_root = cluster.daemon_a.workdir.join("dist");
    std::fs::create_dir_all(source_root.join("empty")).unwrap();
    std::fs::create_dir_all(source_root.join("bin")).unwrap();
    std::fs::write(source_root.join("bin/tool.sh"), "#!/bin/sh\necho hi\n").unwrap();
    let mut perms = std::fs::metadata(source_root.join("bin/tool.sh"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(source_root.join("bin/tool.sh"), perms).unwrap();
    let destination = cluster.daemon_b.workdir.join("release");

    let result = cluster
        .broker
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-a",
                    "path": source_root.display().to_string()
                },
                "destination": {
                    "target": "builder-b",
                    "path": destination.display().to_string()
                },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert!(destination.join("empty").is_dir());
    assert_eq!(
        std::fs::metadata(destination.join("bin/tool.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    assert!(!destination.join("dist").exists());
    assert_eq!(result.structured_content["source_type"], "directory");
}
```

- [ ] **Step 2: Run the focused end-to-end verification and fix any remaining relay issues**

Run: `cargo test -p remote-exec-broker --test multi_target transfer_files -- --nocapture`
Expected: FAIL first if any broker relay branch, daemon identity refresh, or exact-path behavior is still incorrect.

- [ ] **Step 3: Update the README for the new public tool and broker-host trust model**

```markdown
<!-- README.md -->

## Supported tools

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`

## Reliability Notes

- `transfer_files` uses broker-mediated copy for `local -> remote`, `remote -> local`, `remote -> remote`, and `local -> local`.
- `transfer_files` treats `destination.path` as the exact final path to create or replace; it does not infer basenames or copy "into" an existing directory.

## Quality Gate

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Focused transfer commands:

```bash
cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture
cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture
cargo test -p remote-exec-broker --test multi_target -- --nocapture
```

## Trust model

Selecting `target: "local"` in `transfer_files` is equivalent to full filesystem access on the broker host.

Configured remote targets may not be named `local`.
```

- [ ] **Step 4: Run the targeted transfer suite**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: PASS with export/import daemon coverage.

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: PASS with public broker-surface `local -> local` coverage.

Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
Expected: PASS with real `local -> remote`, `remote -> local`, and `remote -> remote` transfers.

- [ ] **Step 5: Run the full workspace verification**

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add tests/e2e/multi_target.rs README.md
git commit -m "docs: document transfer_files workflow"
```
