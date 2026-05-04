# Transfer Files Exclude Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Add source-root-relative `exclude` glob support to `transfer_files` across the broker, broker-host `local`, the Rust daemon, and the C++ daemon without changing import semantics or transfer result fields.

**Architecture:** Keep exclusion as an export-side capability. The public tool and broker-daemon export RPC gain an `exclude` list, the broker forwards that list unchanged for each source, the Rust path compiles normalized glob matchers before export traversal, and the C++ path implements the same bounded grammar in an internal matcher used by directory export. Single-file source behavior stays unchanged in v1: excludes apply only to descendants beneath directory roots.

**Tech Stack:** Rust 2024, Tokio, axum, reqwest, rmcp, serde/schemars, tar, globset, C++17, existing C++ test harnesses, cargo test, make

---

## File Map

- `Cargo.toml`
  - Add `globset` as a workspace dependency for the Rust daemon exclude matcher.
- `crates/remote-exec-proto/src/public.rs`
  - Add the public `exclude` field to `TransferFilesInput`.
- `crates/remote-exec-proto/src/rpc.rs`
  - Add the export-RPC `exclude` field to `TransferExportRequest`.
- `crates/remote-exec-broker/src/tools/transfer.rs`
  - Thread the new `exclude` field from public input into single-source and multi-source transfer operations.
- `crates/remote-exec-broker/src/tools/transfer/operations.rs`
  - Copy `exclude` into every per-source `TransferExportRequest`.
- `crates/remote-exec-broker/src/bin/remote_exec.rs`
  - Add repeated `--exclude <glob>` CLI flags and populate the public input field.
- `crates/remote-exec-broker/tests/support/stub_daemon.rs`
  - Capture the last `TransferExportRequest` so broker tests can assert forwarding.
- `crates/remote-exec-broker/tests/support/fixture.rs`
  - Expose a `last_transfer_export()` helper.
- `crates/remote-exec-broker/tests/mcp_transfer.rs`
  - Add broker-surface tests for forwarding and local directory exclusion behavior.
- `crates/remote-exec-daemon/Cargo.toml`
  - Pull in `globset` from the workspace.
- `crates/remote-exec-daemon/src/transfer/mod.rs`
  - Pass `req.exclude` into the archive export path.
- `crates/remote-exec-daemon/src/transfer/archive/export.rs`
  - Compile the exclude matcher once per export and consult it before archiving files or descending into directories.
- `crates/remote-exec-daemon/src/transfer/archive/exclude_matcher.rs`
  - New Rust helper for pattern normalization, validation, and matching.
- `crates/remote-exec-daemon/src/transfer/archive/mod.rs`
  - Export the new matcher helper if needed by `export.rs`.
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
  - Add export tests for glob matching, malformed patterns, silent omission, and single-file behavior.
- `crates/remote-exec-daemon-cpp/Makefile`
  - Compile the new matcher source into host and XP targets.
- `crates/remote-exec-daemon-cpp/include/transfer_ops.h`
  - Extend export entry points to accept `exclude` patterns.
- `crates/remote-exec-daemon-cpp/src/server.cpp`
  - Parse `exclude` from transfer export JSON and pass it into export helpers before streaming begins.
- `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
  - Extend `ExportOptions` and declare matcher helpers.
- `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`
  - Compile the matcher, normalize relative paths, and prune matching directories before recursion.
- `crates/remote-exec-daemon-cpp/src/transfer_glob.h`
  - New header for the bounded C++ glob grammar.
- `crates/remote-exec-daemon-cpp/src/transfer_glob.cpp`
  - New C++ matcher implementation for `*`, `?`, `**`, and character classes.
- `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
  - Direct export tests for matching, pruning, malformed classes, and single-file behavior.
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
  - Route-level JSON parsing and error tests for `exclude`.
- `README.md`
  - Document the new public field and supported glob grammar.
- `crates/remote-exec-daemon-cpp/README.md`
  - Document parity for C++ transfer exclusion behavior.
- `skills/using-remote-exec-mcp/SKILL.md`
  - Update `transfer_files` usage examples and behavior notes.

### Task 1: Wire The Public Contract And Broker Forwarding

**Files:**
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer.rs`
- Modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Modify: `crates/remote-exec-broker/src/bin/remote_exec.rs`
- Modify: `crates/remote-exec-broker/tests/support/stub_daemon.rs`
- Modify: `crates/remote-exec-broker/tests/support/fixture.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-broker transfer_files_forwards_exclude_patterns_to_remote_exports -- --nocapture`

**Testing approach:** `TDD`
Reason: the broker forwarding contract is a clean public seam. A failing broker integration test can prove the new field is absent before any transfer implementation changes land.

- [ ] **Step 1: Add the failing broker integration test and export-request capture**

```rust
// crates/remote-exec-broker/tests/support/stub_daemon.rs

#[derive(Debug, Clone)]
pub struct StubTransferExportCapture {
    pub request: TransferExportRequest,
}

#[derive(Clone)]
pub(super) struct StubDaemonState {
    pub(super) target: String,
    pub(super) daemon_instance_id: Arc<Mutex<String>>,
    pub(super) exec_write_behavior: Arc<Mutex<ExecWriteBehavior>>,
    pub(super) exec_start_behavior: Arc<Mutex<ExecStartBehavior>>,
    pub(super) exec_start_warnings: Arc<Mutex<Vec<ExecWarning>>>,
    pub(super) exec_start_calls: Arc<Mutex<usize>>,
    pub(super) last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
    pub(super) last_transfer_import: Arc<Mutex<Option<StubTransferImportCapture>>>,
    pub(super) last_transfer_export: Arc<Mutex<Option<StubTransferExportCapture>>>,
    pub(super) image_read_response: Arc<Mutex<StubImageReadResponse>>,
    transfer_export_response: Arc<Mutex<StubTransferExportResponse>>,
    transfer_path_info_response: Arc<Mutex<TransferPathInfoResponse>>,
}

pub(super) fn stub_daemon_state(
    target: &str,
    exec_write_behavior: ExecWriteBehavior,
    platform: &str,
    supports_pty: bool,
) -> StubDaemonState {
    StubDaemonState {
        target: target.to_string(),
        daemon_instance_id: Arc::new(Mutex::new("daemon-instance-1".to_string())),
        exec_write_behavior: Arc::new(Mutex::new(exec_write_behavior)),
        exec_start_behavior: Arc::new(Mutex::new(ExecStartBehavior::Success)),
        exec_start_warnings: Arc::new(Mutex::new(Vec::new())),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
        last_transfer_import: Arc::new(Mutex::new(None)),
        last_transfer_export: Arc::new(Mutex::new(None)),
        image_read_response: Arc::new(Mutex::new(StubImageReadResponse::Success(
            ImageReadResponse {
                image_url: "data:image/png;base64,AAAA".to_string(),
                detail: None,
            },
        ))),
        transfer_export_response: Arc::new(Mutex::new(StubTransferExportResponse::Success {
            source_type: TransferSourceType::Directory,
            compression: TransferCompression::None,
            body: stub_directory_archive(),
        })),
        transfer_path_info_response: Arc::new(Mutex::new(TransferPathInfoResponse {
            exists: false,
            is_directory: false,
        })),
    }
}

async fn transfer_export(
    State(state): State<StubDaemonState>,
    Json(req): Json<TransferExportRequest>,
) -> Result<(HeaderMap, Vec<u8>), (StatusCode, Json<RpcErrorBody>)> {
    *state.last_transfer_export.lock().await = Some(StubTransferExportCapture {
        request: req.clone(),
    });
    match state.transfer_export_response.lock().await.clone() {
        StubTransferExportResponse::Success {
            source_type,
            compression,
            body,
        } => {
            let mut headers = HeaderMap::new();
            headers.insert(
                TRANSFER_SOURCE_TYPE_HEADER,
                HeaderValue::from_static(match source_type {
                    TransferSourceType::File => "file",
                    TransferSourceType::Directory => "directory",
                    TransferSourceType::Multiple => "multiple",
                }),
            );
            headers.insert(
                TRANSFER_COMPRESSION_HEADER,
                HeaderValue::from_static(match compression {
                    TransferCompression::None => "none",
                    TransferCompression::Zstd => "zstd",
                }),
            );
            Ok((headers, body))
        }
        StubTransferExportResponse::Error { status, body } => Err((status, Json(body))),
    }
}

// crates/remote-exec-broker/tests/support/fixture.rs

use super::stub_daemon::{
    ExecStartBehavior, ExecWriteBehavior, StubDaemonState, StubImageReadResponse,
    StubTransferExportCapture, StubTransferImportCapture, set_transfer_export_directory_response,
    set_transfer_export_file_response, set_transfer_path_info_response,
};

impl BrokerFixture {
    pub async fn last_transfer_export(&self) -> Option<StubTransferExportCapture> {
        self.stub_state.last_transfer_export.lock().await.clone()
    }
}

// crates/remote-exec-broker/tests/mcp_transfer.rs

#[tokio::test]
async fn transfer_files_forwards_exclude_patterns_to_remote_exports() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let destination = fixture._tempdir.path().join("copied");

    fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": {
                    "target": "builder-xp",
                    "path": "/srv/reports"
                },
                "destination": {
                    "target": "local",
                    "path": destination.display().to_string()
                },
                "exclude": ["**/*.log", ".git/**"],
                "create_parent": true
            }),
        )
        .await;

    let capture = fixture
        .last_transfer_export()
        .await
        .expect("transfer export capture");
    assert_eq!(capture.request.exclude, vec!["**/*.log", ".git/**"]);
}
```

- [ ] **Step 2: Run the focused broker test and confirm the contract is still missing**

Run: `cargo test -p remote-exec-broker transfer_files_forwards_exclude_patterns_to_remote_exports -- --nocapture`
Expected: FAIL because `TransferExportRequest` does not yet have an `exclude` field and the broker does not forward it.

- [ ] **Step 3: Add the public field, RPC field, broker plumbing, and CLI flag**

```rust
// crates/remote-exec-proto/src/public.rs

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransferFilesInput {
    #[serde(default)]
    pub source: Option<TransferEndpoint>,
    #[serde(default)]
    pub sources: Vec<TransferEndpoint>,
    pub destination: TransferEndpoint,
    #[serde(default)]
    pub overwrite: TransferOverwrite,
    #[serde(default)]
    pub destination_mode: TransferDestinationMode,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub create_parent: bool,
}

// crates/remote-exec-proto/src/rpc.rs

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferExportRequest {
    pub path: String,
    #[serde(default, skip_serializing_if = "TransferCompression::is_none")]
    pub compression: TransferCompression,
    #[serde(default)]
    pub symlink_mode: TransferSymlinkMode,
    #[serde(default)]
    pub exclude: Vec<String>,
}

// crates/remote-exec-broker/src/tools/transfer.rs

let (source_type, summary) = match sources.as_slice() {
    [source] => {
        transfer_single_source(
            state,
            source,
            &destination,
            &input.overwrite,
            &compression,
            &input.symlink_mode,
            &input.exclude,
            input.create_parent,
        )
        .await?
    }
    _ => {
        transfer_multiple_sources(
            state,
            &sources,
            &destination,
            &input.overwrite,
            &compression,
            &input.symlink_mode,
            &input.exclude,
            input.create_parent,
        )
        .await?
    }
};

// crates/remote-exec-broker/src/tools/transfer/operations.rs

pub(super) async fn transfer_single_source(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
    exclude: &[String],
    create_parent: bool,
) -> anyhow::Result<(RpcTransferSourceType, TransferImportResponse)> {
    match (source.target.as_str(), destination.target.as_str()) {
        ("local", "local") => {
            let export_request = build_export_request(source, compression, symlink_mode, exclude);
            let exported = crate::local_transfer::export_path_to_stream(
                &source.path,
                &export_request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            let request = build_import_request(
                destination,
                overwrite,
                exported.source_type.clone(),
                compression,
                symlink_mode,
                create_parent,
            );
            let summary = crate::local_transfer::import_archive_from_async_reader(
                exported.reader,
                &request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            Ok((exported.source_type, summary))
        }
        ("local", target_name) => {
            let export_request = build_export_request(source, compression, symlink_mode, exclude);
            let exported = crate::local_transfer::export_path_to_stream(
                &source.path,
                &export_request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            let request = build_import_request(
                destination,
                overwrite,
                exported.source_type.clone(),
                compression,
                symlink_mode,
                create_parent,
            );
            let body =
                reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(exported.reader));
            let summary =
                import_remote_body_to_endpoint(state, target_name, body, &request).await?;
            Ok((exported.source_type, summary))
        }
        (target_name, "local") => {
            let export_request = build_export_request(source, compression, symlink_mode, exclude);
            let target = verified_remote_target(state, target_name).await?;
            let exported = handle_remote_transfer_result(
                target,
                target.transfer_export_stream(&export_request).await,
            )
            .await?;
            let source_type = exported.source_type.clone();
            let request = build_import_request(
                destination,
                overwrite,
                source_type.clone(),
                compression,
                symlink_mode,
                create_parent,
            );
            let summary = crate::local_transfer::import_archive_from_async_reader(
                exported.into_async_read(),
                &request,
                state.host_sandbox.as_ref(),
            )
            .await?;
            Ok((source_type, summary))
        }
        (source_target_name, destination_target_name) => {
            let export_request = build_export_request(source, compression, symlink_mode, exclude);
            let source_target = verified_remote_target(state, source_target_name).await?;
            let exported = handle_remote_transfer_result(
                source_target,
                source_target.transfer_export_stream(&export_request).await,
            )
            .await?;
            let source_type = exported.source_type.clone();
            let request = build_import_request(
                destination,
                overwrite,
                source_type.clone(),
                compression,
                symlink_mode,
                create_parent,
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

pub(super) async fn transfer_multiple_sources(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
    exclude: &[String],
    create_parent: bool,
) -> anyhow::Result<(RpcTransferSourceType, TransferImportResponse)> {
    let mut exported_sources = Vec::with_capacity(sources.len());
    for source in sources {
        let temp = tempfile::NamedTempFile::new()?;
        let temp_path = temp.into_temp_path();
        let source_policy = endpoint_policy(state, source).await?;
        let exported = export_endpoint_to_archive(
            state,
            source,
            temp_path.as_ref(),
            compression,
            symlink_mode,
            exclude,
        )
        .await?;
        exported_sources.push(ExportedSourceArchive {
            endpoint: source.clone(),
            source_policy,
            source_type: exported.source_type,
            temp_path,
        });
    }
}

fn build_export_request(
    endpoint: &TransferEndpoint,
    compression: &RpcTransferCompression,
    symlink_mode: &PublicTransferSymlinkMode,
    exclude: &[String],
) -> TransferExportRequest {
    TransferExportRequest {
        path: endpoint.path.clone(),
        compression: compression.clone(),
        symlink_mode: to_rpc_symlink_mode(symlink_mode),
        exclude: exclude.to_vec(),
    }
}

// crates/remote-exec-broker/src/bin/remote_exec.rs

#[derive(Args, Debug)]
struct TransferFilesArgs {
    #[arg(long = "source", required = true)]
    sources: Vec<String>,

    #[arg(long)]
    destination: String,

    #[arg(long = "exclude")]
    exclude: Vec<String>,

    #[arg(long, value_enum, default_value_t = CliTransferOverwrite::Merge)]
    overwrite: CliTransferOverwrite,
    #[arg(long, value_enum, default_value_t = CliTransferDestinationMode::Auto)]
    destination_mode: CliTransferDestinationMode,

    #[arg(long, value_enum, default_value_t = CliTransferSymlinkMode::Preserve)]
    symlink_mode: CliTransferSymlinkMode,

    #[arg(long, default_value_t = false)]
    create_parent: bool,
}

fn transfer_files_input(args: TransferFilesArgs) -> anyhow::Result<TransferFilesInput> {
    let endpoints = args
        .sources
        .iter()
        .map(|endpoint| parse_transfer_endpoint(endpoint))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(TransferFilesInput {
        source: (endpoints.len() == 1).then(|| endpoints[0].clone()),
        sources: if endpoints.len() == 1 { Vec::new() } else { endpoints },
        destination: parse_transfer_endpoint(&args.destination)?,
        overwrite: args.overwrite.into(),
        destination_mode: args.destination_mode.into(),
        symlink_mode: args.symlink_mode.into(),
        exclude: args.exclude,
        create_parent: args.create_parent,
    })
}
```

- [ ] **Step 4: Run the focused broker verification**

Run: `cargo test -p remote-exec-broker transfer_files_forwards_exclude_patterns_to_remote_exports -- --nocapture`
Expected: PASS, with the stub daemon capturing the forwarded `exclude` list exactly.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-proto/src/public.rs \
  crates/remote-exec-proto/src/rpc.rs \
  crates/remote-exec-broker/src/tools/transfer.rs \
  crates/remote-exec-broker/src/tools/transfer/operations.rs \
  crates/remote-exec-broker/src/bin/remote_exec.rs \
  crates/remote-exec-broker/tests/support/stub_daemon.rs \
  crates/remote-exec-broker/tests/support/fixture.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "feat: forward transfer excludes through broker exports"
```

### Task 2: Add The Rust Exclude Matcher Helper

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/remote-exec-daemon/Cargo.toml`
- Create: `crates/remote-exec-daemon/src/transfer/archive/exclude_matcher.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/archive/mod.rs`
- Test/Verify: `cargo test -p remote-exec-daemon matches_double_star_and_negated_classes -- --nocapture`

**Testing approach:** `TDD`
Reason: the grammar and normalization rules are easiest to lock down with pure matcher unit tests before touching filesystem traversal.

- [ ] **Step 1: Add failing matcher unit tests for normalization, negated classes, and malformed patterns**

```rust
// crates/remote-exec-daemon/src/transfer/archive/exclude_matcher.rs

#[cfg(test)]
mod tests {
    use super::ExcludeMatcher;
    use std::path::Path;

    fn compile(patterns: &[&str]) -> ExcludeMatcher {
        ExcludeMatcher::compile(
            &patterns.iter().map(|pattern| pattern.to_string()).collect::<Vec<_>>(),
        )
        .expect("compile matcher")
    }

    #[test]
    fn matches_double_star_and_negated_classes() {
        let matcher = compile(&["**/*.log", "[!a-c].txt", "[^abc].cfg", "build/[a-z]*.tmp"]);
        assert!(matcher.matches_relative_path(Path::new("logs/run.log")));
        assert!(matcher.matches_relative_path(Path::new("z.txt")));
        assert!(matcher.matches_relative_path(Path::new("z.cfg")));
        assert!(matcher.matches_relative_path(Path::new("build/cache.tmp")));
        assert!(!matcher.matches_relative_path(Path::new("b.txt")));
        assert!(!matcher.matches_relative_path(Path::new("a.cfg")));
    }

    #[test]
    fn normalizes_backslashes_and_caret_negation() {
        let matcher = compile(&[r"dir\[^a-c].txt"]);
        assert!(matcher.matches_relative_path(Path::new("dir/z.txt")));
        assert!(!matcher.matches_relative_path(Path::new("dir/b.txt")));
    }

    #[test]
    fn rejects_empty_patterns() {
        let err = ExcludeMatcher::compile(&["".to_string()]).unwrap_err();
        assert!(err.to_string().contains("exclude pattern must not be empty"));
    }

    #[test]
    fn rejects_malformed_character_classes() {
        let err = ExcludeMatcher::compile(&["[".to_string()]).unwrap_err();
        assert!(err.to_string().contains("invalid exclude pattern"));
    }
}
```

- [ ] **Step 2: Run the focused matcher test and confirm the helper does not exist yet**

Run: `cargo test -p remote-exec-daemon matches_double_star_and_negated_classes -- --nocapture`
Expected: FAIL because `exclude_matcher.rs` and `ExcludeMatcher` do not exist.

- [ ] **Step 3: Add the dependency and implement the Rust matcher helper**

```toml
# Cargo.toml

[workspace.dependencies]
globset = "0.4"
```

```toml
# crates/remote-exec-daemon/Cargo.toml

[dependencies]
globset = { workspace = true }
```

```rust
// crates/remote-exec-daemon/src/transfer/archive/mod.rs

pub mod exclude_matcher;

// crates/remote-exec-daemon/src/transfer/archive/exclude_matcher.rs

use std::path::Path;

use anyhow::Context;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

#[derive(Debug, Clone)]
pub struct ExcludeMatcher {
    set: Option<GlobSet>,
}

impl ExcludeMatcher {
    pub fn compile(patterns: &[String]) -> anyhow::Result<Self> {
        if patterns.is_empty() {
            return Ok(Self { set: None });
        }

        let mut builder = GlobSetBuilder::new();
        for raw in patterns {
            let normalized = normalize_pattern(raw)?;
            let glob = GlobBuilder::new(&normalized)
                .literal_separator(true)
                .backslash_escape(false)
                .build()
                .with_context(|| format!("invalid exclude pattern `{raw}`"))?;
            builder.add(glob);
        }

        Ok(Self {
            set: Some(builder.build().context("building exclude matcher")?),
        })
    }

    pub fn matches_relative_path(&self, relative: &Path) -> bool {
        let Some(set) = &self.set else {
            return false;
        };
        let candidate = normalize_relative_path(relative);
        if candidate.is_empty() {
            return false;
        }
        set.is_match(candidate)
    }
}

fn normalize_pattern(raw: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        !raw.is_empty(),
        "exclude pattern must not be empty"
    );
    rewrite_negated_classes(&raw.replace('\\', "/"))
}

fn normalize_relative_path(relative: &Path) -> String {
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
}

fn rewrite_negated_classes(pattern: &str) -> anyhow::Result<String> {
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut out = String::with_capacity(pattern.len());
    let mut idx = 0usize;
    while idx < chars.len() {
        if chars[idx] != '[' {
            out.push(chars[idx]);
            idx += 1;
            continue;
        }

        let start = idx;
        out.push('[');
        idx += 1;
        anyhow::ensure!(idx < chars.len(), "invalid exclude pattern `{pattern}`");
        if chars[idx] == '^' {
            out.push('!');
            idx += 1;
        } else {
            out.push(chars[idx]);
            idx += 1;
        }

        let mut closed = false;
        while idx < chars.len() {
            let ch = chars[idx];
            out.push(ch);
            idx += 1;
            if ch == ']' {
                closed = true;
                break;
            }
        }

        anyhow::ensure!(
            closed,
            "invalid exclude pattern `{}`",
            chars[start..].iter().collect::<String>()
        );
    }
    Ok(out)
}
```

- [ ] **Step 4: Run the post-change matcher verification**

Run: `cargo test -p remote-exec-daemon matches_double_star_and_negated_classes -- --nocapture`
Expected: PASS, with the helper accepting `!` and `^` negation forms after normalization.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml \
  crates/remote-exec-daemon/Cargo.toml \
  crates/remote-exec-daemon/src/transfer/archive/mod.rs \
  crates/remote-exec-daemon/src/transfer/archive/exclude_matcher.rs
git commit -m "feat: add rust transfer exclude matcher"
```

### Task 3: Integrate Rust Export Filtering And Broker-Local Behavior

**Files:**
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/archive/export.rs`
- Modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker transfer_files_excludes_local_directory_entries -- --nocapture`

**Testing approach:** `TDD`
Reason: export traversal behavior has a clear daemon RPC seam, and broker-host `local` reuses that same export path, so one set of failing tests can drive both.

- [ ] **Step 1: Add failing Rust daemon export tests and a broker local-to-local behavior test**

```rust
// crates/remote-exec-daemon/tests/transfer_rpc.rs

#[tokio::test]
async fn export_directory_applies_exclude_patterns() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(root.join("logs")).await.unwrap();
    tokio::fs::create_dir_all(root.join("src")).await.unwrap();
    tokio::fs::write(root.join("logs/run.log"), "skip\n").await.unwrap();
    tokio::fs::write(root.join("src/app.txt"), "keep\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: vec!["**/*.log".to_string()],
            },
        )
        .await;

    assert!(response.status().is_success());
    let bytes = response.bytes().await.unwrap().to_vec();
    let mut archive = tar::Archive::new(std::io::Cursor::new(bytes));
    let paths = archive
        .entries()
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();

    assert!(paths.iter().any(|path| path == "src/app.txt"));
    assert!(!paths.iter().any(|path| path == "logs/run.log"));
}

#[tokio::test]
async fn export_directory_rejects_malformed_exclude_patterns() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let root = fixture.workdir.join("dist");
    tokio::fs::create_dir_all(&root).await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: root.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: vec!["[".to_string()],
            },
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body = response.json::<remote_exec_proto::rpc::RpcErrorBody>().await.unwrap();
    assert!(body.message.contains("invalid exclude pattern"));
}

#[tokio::test]
async fn export_file_ignores_exclude_patterns_in_v1() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let source = fixture.workdir.join("hello.txt");
    tokio::fs::write(&source, "hello\n").await.unwrap();

    let response = fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: source.display().to_string(),
                compression: TransferCompression::None,
                symlink_mode: Default::default(),
                exclude: vec!["*.txt".to_string()],
            },
        )
        .await;

    assert!(response.status().is_success());
}

// crates/remote-exec-broker/tests/mcp_transfer.rs

#[tokio::test]
async fn transfer_files_excludes_local_directory_entries() {
    let fixture = support::spawners::spawn_broker_with_stub_daemon().await;
    let source = fixture._tempdir.path().join("source");
    let destination = fixture._tempdir.path().join("dest");
    std::fs::create_dir_all(source.join("logs")).unwrap();
    std::fs::create_dir_all(source.join("src")).unwrap();
    std::fs::write(source.join("logs/run.log"), "skip\n").unwrap();
    std::fs::write(source.join("src/app.txt"), "keep\n").unwrap();

    fixture
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
                "exclude": ["**/*.log"],
                "create_parent": false
            }),
        )
        .await;

    assert!(destination.join("src/app.txt").exists());
    assert!(!destination.join("logs/run.log").exists());
}
```

- [ ] **Step 2: Run the focused Rust export verification and confirm the traversal still copies everything**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: FAIL because `TransferExportRequest` is not used during traversal and excluded entries are still archived.

- [ ] **Step 3: Thread excludes into Rust export preparation and consult the matcher during traversal**

```rust
// crates/remote-exec-daemon/src/transfer/mod.rs

let exported = archive::export_path_to_stream(
    &req.path,
    req.compression.clone(),
    req.symlink_mode.clone(),
    &req.exclude,
    state.sandbox.as_ref(),
    state.config.windows_posix_root.as_deref(),
)
.await
.map_err(map_transfer_error)?;

// crates/remote-exec-broker/src/local_transfer.rs

let exported = remote_exec_daemon::transfer::archive::export_path_to_file(
    path,
    archive_path,
    request.compression.clone(),
    request.symlink_mode.clone(),
    &request.exclude,
    sandbox,
    None,
)
.await?;

let exported = remote_exec_daemon::transfer::archive::export_path_to_stream(
    path,
    request.compression.clone(),
    request.symlink_mode.clone(),
    &request.exclude,
    sandbox,
    None,
)
.await?;

// crates/remote-exec-daemon/src/transfer/archive/export.rs

use super::exclude_matcher::ExcludeMatcher;

struct PreparedExport {
    source_path: PathBuf,
    source_type: TransferSourceType,
    exclude_matcher: ExcludeMatcher,
}

pub async fn export_path_to_file(
    path: &str,
    archive_path: &Path,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<ExportPathResult> {
    let prepared =
        prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root).await?;
    let archive_path = archive_path.to_path_buf();
    let source_type = prepared.source_type.clone();
    let warnings =
        write_prepared_export_to_file(prepared, archive_path, compression, symlink_mode).await?;
    Ok(ExportPathResult {
        source_type,
        warnings,
    })
}

pub async fn export_path_to_stream(
    path: &str,
    compression: TransferCompression,
    symlink_mode: TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<ExportedArchiveStream> {
    let prepared =
        prepare_export_path(path, &symlink_mode, exclude, sandbox, windows_posix_root).await?;
    let source_type = prepared.source_type.clone();
    let (reader, writer) = tokio::io::duplex(STREAM_BUFFER_SIZE);
    let task_compression = compression.clone();
    tokio::spawn(async move {
        let writer = tokio_util::io::SyncIoBridge::new(writer);
        if let Err(err) =
            write_prepared_export_to_writer(prepared, writer, task_compression, symlink_mode).await
        {
            tracing::debug!(error = %err, "streamed transfer export stopped");
        }
    });

    Ok(ExportedArchiveStream {
        source_type,
        compression,
        reader,
    })
}

async fn prepare_export_path(
    path: &str,
    symlink_mode: &TransferSymlinkMode,
    exclude: &[String],
    sandbox: Option<&CompiledFilesystemSandbox>,
    windows_posix_root: Option<&Path>,
) -> anyhow::Result<PreparedExport> {
    let source_text = path.to_string();
    anyhow::ensure!(
        crate::host_path::is_input_path_absolute(&source_text, windows_posix_root),
        "transfer source path `{source_text}` is not absolute"
    );
    let source_path = host_path(&source_text, windows_posix_root)?;
    authorize_path(host_policy(), sandbox, SandboxAccess::Read, &source_path)?;
    let metadata = tokio::fs::symlink_metadata(&source_path).await?;
    let source_type = export_source_type_from_metadata(&source_path, &metadata, symlink_mode)?;
    Ok(PreparedExport {
        source_path,
        source_type,
        exclude_matcher: ExcludeMatcher::compile(exclude)?,
    })
}

fn append_export_source<W: Write>(
    builder: &mut tar::Builder<W>,
    source_path: &Path,
    source_type: TransferSourceType,
    symlink_mode: &TransferSymlinkMode,
    exclude_matcher: &ExcludeMatcher,
) -> anyhow::Result<Vec<TransferWarning>> {
    let mut warnings = Vec::new();
    match source_type {
        TransferSourceType::File => {
            append_file_or_symlink_entry(
                builder,
                source_path,
                Path::new(SINGLE_FILE_ENTRY),
                symlink_mode,
            )?;
        }
        TransferSourceType::Directory => {
            builder.append_dir(".", source_path)?;
            append_directory_entries(
                builder,
                source_path,
                source_path,
                symlink_mode,
                exclude_matcher,
                &mut warnings,
            )?;
        }
        TransferSourceType::Multiple => anyhow::bail!("single-path export cannot produce a multi-source archive"),
    }
    Ok(warnings)
}

fn append_directory_entries<W: Write>(
    builder: &mut tar::Builder<W>,
    root: &Path,
    current: &Path,
    symlink_mode: &TransferSymlinkMode,
    exclude_matcher: &ExcludeMatcher,
    warnings: &mut Vec<TransferWarning>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(root)?;
        if exclude_matcher.matches_relative_path(rel) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            builder.append_dir(rel, &path)?;
            append_directory_entries(builder, root, &path, symlink_mode, exclude_matcher, warnings)?;
            continue;
        }
        if metadata.file_type().is_symlink() {
            match symlink_mode {
                TransferSymlinkMode::Preserve => {
                    append_symlink_entry(builder, &path, rel)?;
                }
                TransferSymlinkMode::Follow => {
                    let target_metadata = std::fs::metadata(&path)?;
                    if target_metadata.is_dir() {
                        builder.append_dir(rel, &path)?;
                        append_directory_entries(
                            builder,
                            root,
                            &path,
                            symlink_mode,
                            exclude_matcher,
                            warnings,
                        )?;
                    } else if target_metadata.is_file() {
                        builder.append_path_with_name(&path, rel)?;
                    } else {
                        warnings.push(TransferWarning::skipped_unsupported_entry(path.display()));
                    }
                }
                TransferSymlinkMode::Skip => {
                    warnings.push(TransferWarning::skipped_symlink(path.display()));
                }
            }
            continue;
        }
        if metadata.is_file() {
            builder.append_path_with_name(&path, rel)?;
        } else {
            warnings.push(TransferWarning::skipped_unsupported_entry(path.display()));
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run the post-change Rust and broker-local verification**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture`
Expected: PASS, with matching files omitted, malformed patterns rejected, and single-file sources still exported.

Run: `cargo test -p remote-exec-broker transfer_files_excludes_local_directory_entries -- --nocapture`
Expected: PASS, proving broker-host `local` behavior follows the Rust export path.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/transfer/archive/export.rs \
  crates/remote-exec-broker/src/local_transfer.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "feat: apply excludes in rust transfer exports"
```

### Task 4: Add The C++ Exclude Matcher And Export Route Integration

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/Makefile`
- Modify: `crates/remote-exec-daemon-cpp/include/transfer_ops.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`
- Create: `crates/remote-exec-daemon-cpp/src/transfer_glob.h`
- Create: `crates/remote-exec-daemon-cpp/src/transfer_glob.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `TDD`
Reason: the C++ daemon needs parity with Rust, and its direct transfer tests plus route tests are the clearest way to lock down grammar support and pre-header failure behavior.

- [ ] **Step 1: Add failing C++ tests for matching, malformed patterns, and route parsing**

```cpp
// crates/remote-exec-daemon-cpp/tests/test_transfer.cpp

static std::vector<std::string> read_tar_paths(const std::string& archive) {
    std::vector<std::string> paths;
    std::size_t offset = 0;
    while (offset + 512 <= archive.size()) {
        const char* header = archive.data() + offset;
        if (block_is_zero(header)) {
            break;
        }
        std::size_t path_length = 0;
        while (path_length < 100 && header[path_length] != '\0') {
            ++path_length;
        }
        paths.push_back(std::string(header, path_length));
        const std::uint64_t size = parse_octal_value(header + 124, 12);
        offset += 512 + ((static_cast<std::size_t>(size) + 511) / 512) * 512;
    }
    return paths;
}

static void assert_directory_transfer_excludes_matching_entries() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-exclude";
    fs::remove_all(root);
    fs::create_directories(root / "logs");
    fs::create_directories(root / "src");
    fs::create_directories(root / "config");
    write_text(root / "logs" / "run.log", "skip");
    write_text(root / "src" / "app.txt", "keep");
    write_text(root / "z.txt", "skip");
    write_text(root / "b.txt", "keep");
    write_text(root / "config" / "z.cfg", "skip");
    write_text(root / "config" / "a.cfg", "keep");

    const ExportedPayload exported = export_path(
        root.string(),
        "preserve",
        std::vector<std::string>{"**/*.log", "[!a-c].txt", "config/[^abc].cfg"}
    );

    const std::vector<std::string> paths = read_tar_paths(exported.bytes);
    assert(std::find(paths.begin(), paths.end(), "src/app.txt") != paths.end());
    assert(std::find(paths.begin(), paths.end(), "logs/run.log") == paths.end());
    assert(std::find(paths.begin(), paths.end(), "z.txt") == paths.end());
    assert(std::find(paths.begin(), paths.end(), "b.txt") != paths.end());
    assert(std::find(paths.begin(), paths.end(), "config/z.cfg") == paths.end());
    assert(std::find(paths.begin(), paths.end(), "config/a.cfg") != paths.end());
}

static void assert_export_rejects_invalid_exclude_pattern() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-bad-exclude";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)export_path(root.string(), "preserve", std::vector<std::string>{"["});
    } catch (const std::exception& ex) {
        rejected = std::string(ex.what()).find("invalid exclude pattern") != std::string::npos;
    }

    assert(rejected);
}

// crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp

const fs::path source_dir = root / "exclude-dir";
fs::create_directories(source_dir / "logs");
fs::create_directories(source_dir / "src");
write_text_file(source_dir / "logs" / "run.log", "skip");
write_text_file(source_dir / "src" / "app.txt", "keep");

const HttpResponse excluded_export = route_request(
    state,
    json_request(
        "/v1/transfer/export",
        Json{
            {"path", source_dir.string()},
            {"exclude", Json::array({"**/*.log", "[!a-c].txt"})}
        }
    )
);
assert(excluded_export.status == 200);

const HttpResponse bad_exclude_export = route_request(
    state,
    json_request(
        "/v1/transfer/export",
        Json{
            {"path", source_dir.string()},
            {"exclude", Json::array({"["})}
        }
    )
);
assert(bad_exclude_export.status == 400);
assert(
    Json::parse(bad_exclude_export.body).at("message").get<std::string>().find("invalid exclude pattern") !=
    std::string::npos
);
```

- [ ] **Step 2: Run the focused host tests and confirm the C++ export path does not yet understand excludes**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: FAIL because `export_path` and related declarations do not accept `exclude` arguments.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: FAIL because `/v1/transfer/export` ignores the JSON `exclude` field.

- [ ] **Step 3: Add the matcher helper, extend export signatures, and route excludes into traversal**

```make
# crates/remote-exec-daemon-cpp/Makefile

TRANSFER_SRCS := $(addprefix $(MAKEFILE_DIR),src/transfer_ops.cpp src/transfer_ops_fs.cpp src/transfer_ops_tar.cpp src/transfer_ops_export.cpp src/transfer_ops_import.cpp src/transfer_glob.cpp)
```

```cpp
// crates/remote-exec-daemon-cpp/include/transfer_ops.h

ExportedPayload export_path(
    const std::string& absolute_path,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
std::string export_path_source_type(
    const std::string& absolute_path,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
std::string export_path_to_sink(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
void export_path_to_sink_as(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& source_type,
    const std::string& symlink_mode = "preserve",
    const std::vector<std::string>& exclude = std::vector<std::string>()
);
```

```cpp
// crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h

struct ExportOptions {
    std::string symlink_mode;
    std::vector<std::string> exclude;
};
```

```cpp
// crates/remote-exec-daemon-cpp/src/transfer_glob.h

#pragma once

#include <string>
#include <vector>

namespace transfer_glob {

class ExcludeMatcher {
public:
    static ExcludeMatcher compile(const std::vector<std::string>& patterns);
    bool matches(const std::string& relative_path) const;

private:
    struct Pattern;
    std::vector<Pattern> patterns_;
};

std::string normalize_pattern(const std::string& raw);
std::string normalize_relative_path(const std::string& raw);

}  // namespace transfer_glob
```

```cpp
// crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp

#include "transfer_glob.h"

struct ExportContext {
    ExportOptions options;
    transfer_glob::ExcludeMatcher exclude_matcher;
    std::vector<TransferWarning> warnings;
    std::set<std::string> followed_directories;
};

void append_directory_contents(
    TransferArchiveSink* archive,
    const std::string& current_path,
    const std::string& current_rel,
    ExportContext* context
) {
    const std::vector<DirectoryEntry> entries = list_directory_entries(current_path);
    for (std::size_t i = 0; i < entries.size(); ++i) {
        const DirectoryEntry& entry = entries[i];
        const std::string child_path = join_path(current_path, entry.name);
        const std::string child_rel =
            current_rel.empty() ? entry.name : current_rel + "/" + entry.name;
        if (context->exclude_matcher.matches(child_rel)) {
            continue;
        }

        if (entry.is_directory) {
            append_directory_entry(archive, child_rel);
            append_directory_contents(archive, child_path, child_rel, context);
            continue;
        }
        if (entry.is_symlink) {
            if (context->options.symlink_mode == "skip") {
                handle_skipped_symlink(context, child_path);
                continue;
            }
#ifdef _WIN32
            if (context->options.symlink_mode == "follow" &&
                append_followed_symlink_entry(archive, child_path, child_rel, context)) {
                continue;
            }
            handle_skipped_symlink(context, child_path);
            continue;
#else
            if (context->options.symlink_mode == "preserve") {
                append_preserved_symlink_entry(archive, child_path, child_rel);
                continue;
            }
            if (context->options.symlink_mode == "follow") {
                if (append_followed_symlink_entry(archive, child_path, child_rel, context)) {
                    continue;
                }
                handle_unsupported_entry(context, child_path);
                continue;
            }
            throw std::runtime_error("transfer source contains unsupported symlink " + child_path);
#endif
        }
        if (!entry.is_regular_file) {
            handle_unsupported_entry(context, child_path);
            continue;
        }
        append_file_entry_from_path(archive, child_rel, child_path);
    }
}

ExportOptions normalized_options(
    const std::string& symlink_mode,
    const std::vector<std::string>& exclude
) {
    ExportOptions options{symlink_mode.empty() ? "preserve" : symlink_mode, exclude};
    validate_transfer_options(options);
    return options;
}

void export_path_to_sink_as(
    TransferArchiveSink& sink,
    const std::string& absolute_path,
    const std::string& source_type,
    const std::string& symlink_mode,
    const std::vector<std::string>& exclude
) {
    ExportContext context;
    context.options = normalized_options(symlink_mode, exclude);
    context.exclude_matcher = transfer_glob::ExcludeMatcher::compile(context.options.exclude);
    validate_export_path(absolute_path, context.options);
    if (source_type == "file") {
        export_file_as_tar(&sink, absolute_path, context.options);
        return;
    }
    if (source_type == "directory") {
        export_directory_as_tar(&sink, absolute_path, &context);
        return;
    }
    throw std::runtime_error("unsupported transfer source type");
}
```

```cpp
// crates/remote-exec-daemon-cpp/src/server.cpp

const Json body_json = parse_json_body(request);
require_uncompressed_transfer(body_json.value("compression", std::string("none")));
const std::vector<std::string> exclude =
    body_json.contains("exclude")
        ? body_json.at("exclude").get<std::vector<std::string>>()
        : std::vector<std::string>();

const std::string source_type = export_path_source_type(path, symlink_mode, exclude);
send_transfer_export_headers(client, source_type);
headers_sent = true;
ChunkedTransferArchiveSink sink(client);
export_path_to_sink_as(sink, path, source_type, symlink_mode, exclude);
```

- [ ] **Step 4: Run the post-change C++ verification**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: PASS, including matching, pruning, and malformed pattern rejection.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS, with invalid patterns rejected before any successful streaming response begins.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-cpp/Makefile \
  crates/remote-exec-daemon-cpp/include/transfer_ops.h \
  crates/remote-exec-daemon-cpp/src/server.cpp \
  crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h \
  crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp \
  crates/remote-exec-daemon-cpp/src/transfer_glob.h \
  crates/remote-exec-daemon-cpp/src/transfer_glob.cpp \
  crates/remote-exec-daemon-cpp/tests/test_transfer.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
git commit -m "feat: add cpp transfer exclude matching"
```

### Task 5: Update Docs And Run Full Verification

**Files:**
- Modify: `README.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer`
- Test/Verify: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp check-posix`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: this task is mostly documentation plus repo-level confidence runs after behavior has already been driven by focused tests in earlier tasks.

- [ ] **Step 1: Update public docs and the remote-exec skill**

```md
<!-- README.md -->
- `transfer_files` accepts an optional `exclude` array of source-root-relative glob patterns.
- Matching always uses `/` as the logical separator on every platform.
- Supported glob syntax: `*`, `?`, `**`, `[abc]`, `[a-z]`, `[!abc]`, `[!a-c]`, `[^abc]`, `[^a-c]`.
- Excluded entries are silently omitted and are not reported as transfer warnings.
- In v1, single-file source transfers ignore `exclude`; only descendants beneath directory roots are matched.

<!-- crates/remote-exec-daemon-cpp/README.md -->
- `transfer_files` export supports the same exclude glob grammar as the Rust daemon for directory traversal.
- Invalid exclude patterns are rejected before the daemon starts streaming the archive body.

<!-- skills/using-remote-exec-mcp/SKILL.md -->
{
  "source": {
    "target": "builder-a",
    "path": "/srv/project"
  },
  "destination": {
    "target": "local",
    "path": "/tmp/project"
  },
  "exclude": [
    ".git/**",
    "**/*.log",
    "build/[!a-c]*.tmp"
  ],
  "create_parent": true
}
```

- [ ] **Step 2: Run the focused verification suite for the changed surface**

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: PASS, including exclude forwarding and broker-host local transfer coverage.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: PASS, including malformed-pattern rejection and exclude matching.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS, including transfer and route tests with the new matcher source compiled in.

- [ ] **Step 3: Run the full workspace quality gate**

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all commands PASS with no formatting drift and no clippy warnings.

- [ ] **Step 4: Commit**

```bash
git add README.md \
  crates/remote-exec-daemon-cpp/README.md \
  skills/using-remote-exec-mcp/SKILL.md
git commit -m "docs: describe transfer file excludes"
```

- [ ] **Step 5: Capture the final changed-file summary for handoff**

```bash
git status --short
git log --oneline -n 5
```

Expected: only the intended tracked files are modified, and the recent commits line up with the task sequence above.
