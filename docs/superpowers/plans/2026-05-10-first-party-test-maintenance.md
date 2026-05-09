# First-Party Test Maintenance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Reduce first-party test maintenance cost across Rust and C++ suites without changing production behavior.

**Architecture:** Work layer by layer: shared transfer test support first, then broker transfer tests, daemon transfer tests, admin/PKI helpers, C++ route/session tests, C++ streaming helpers, and final gates. The plan keeps port-forward and cross-target tests mostly untouched because they were just maintained, and it keeps C++ as a first-class path with focused Makefile verification.

**Tech Stack:** Rust 2024 workspace, Tokio integration tests, rmcp broker harness, shared Rust test support via `#[path]`, C++ assert-plus-Makefile tests, POSIX C++ host test targets.

---

## File Map

- Modify: `tests/support/transfer_archive.rs`
  - Owns reusable archive decode, path reading, single-file archive reading, raw tar construction, multi-source tar construction, and Unix symlink archive construction for first-party Rust tests.
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
  - Uses shared archive helpers and keeps public request-shape tests explicit.
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
  - Uses shared archive helpers and merges simple path-info success cases.
- Modify: `crates/remote-exec-admin/tests/dev_init.rs`
  - Adds local command/assertion helpers for repeated admin CLI workflow setup.
- Modify: `crates/remote-exec-admin/tests/certs_issue.rs`
  - Adds local helpers for CA initialization and successful command assertions.
- Modify: `crates/remote-exec-pki/tests/ca_reuse.rs`
  - Adds local PEM assertion helper.
- Modify: `crates/remote-exec-pki/tests/dev_init_bundle.rs`
  - Adds local daemon bundle assertion helper.
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
  - Extracts route scenario groups into local helper functions while keeping the same executable target.
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
  - Extracts session scenario groups into local helper functions while keeping the same executable target.
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
  - Extracts streaming/tunnel scenario groups into local helper functions while preserving the same target.

Do not edit:

- `third_party/rust-patches/*`
- `winptyrs/*`
- `portable-pty/*`
- generated files under `target/`
- generated files under `crates/remote-exec-daemon-cpp/build/`

### Task 1: Share Rust Transfer Archive Helpers

**Files:**
- Modify: `tests/support/transfer_archive.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`

**Testing approach:** existing tests + targeted verification.
Reason: This task moves duplicated test helper code and updates callers. It should not change production behavior or test assertions.

- [ ] **Step 1: Extend shared archive helpers**

Edit `tests/support/transfer_archive.rs` so it contains these helpers:

```rust
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub const SINGLE_FILE_ENTRY: &str = ".remote-exec-file";

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

pub fn read_single_file_archive(bytes: &[u8]) -> (String, Vec<u8>) {
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut entries = archive.entries().expect("archive entries");
    let mut entry = entries
        .next()
        .expect("archive entry")
        .expect("archive entry ok");
    let path = entry
        .path()
        .expect("entry path")
        .to_string_lossy()
        .into_owned();
    let mut body = Vec::new();
    entry.read_to_end(&mut body).expect("entry body");
    assert!(
        entries
            .next()
            .transpose()
            .expect("no extra entries")
            .is_none(),
        "single-file archive contained extra entries"
    );
    (path, body)
}

pub fn raw_tar_file_with_path(path: impl AsRef<Path>, body: &[u8]) -> Vec<u8> {
    fn write_octal(field: &mut [u8], value: u64) {
        let digits = field.len() - 1;
        let text = format!("{value:o}");
        assert!(
            text.len() <= digits,
            "value {value} does not fit in tar field"
        );
        let start = digits - text.len();
        field[..start].fill(b'0');
        field[start..digits].copy_from_slice(text.as_bytes());
        field[digits] = 0;
    }

    fn write_checksum(field: &mut [u8], checksum: u32) {
        let text = format!("{checksum:o}");
        assert!(
            text.len() <= 6,
            "checksum {checksum} does not fit in tar field"
        );
        let start = 6 - text.len();
        field[..start].fill(b'0');
        field[start..6].copy_from_slice(text.as_bytes());
        field[6] = 0;
        field[7] = b' ';
    }

    let path = path.as_ref().to_string_lossy();
    assert!(
        path.len() <= 100,
        "tar test helper only supports short paths"
    );
    let mut header = [0u8; 512];
    header[..path.len()].copy_from_slice(path.as_bytes());
    write_octal(&mut header[100..108], 0o644);
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], body.len() as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| *byte as u32).sum();
    write_checksum(&mut header[148..156], checksum);

    let mut archive = Vec::with_capacity(512 + body.len() + 1024);
    archive.extend_from_slice(&header);
    archive.extend_from_slice(body);
    let padding = (512 - (body.len() % 512)) % 512;
    archive.resize(archive.len() + padding, 0);
    archive.extend_from_slice(&[0u8; 1024]);
    archive
}

pub fn multi_source_tar() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    let file_body = b"alpha\n";
    let mut alpha = tar::Header::new_gnu();
    alpha.set_entry_type(tar::EntryType::Regular);
    alpha.set_mode(0o644);
    alpha.set_size(file_body.len() as u64);
    alpha.set_cksum();
    builder
        .append_data(
            &mut alpha,
            "alpha.txt",
            std::io::Cursor::new(file_body.as_slice()),
        )
        .unwrap();

    let mut nested = tar::Header::new_gnu();
    nested.set_entry_type(tar::EntryType::Directory);
    nested.set_mode(0o755);
    nested.set_size(0);
    nested.set_cksum();
    builder
        .append_data(&mut nested, "nested", std::io::empty())
        .unwrap();

    let nested_body = b"beta\n";
    let mut beta = tar::Header::new_gnu();
    beta.set_entry_type(tar::EntryType::Regular);
    beta.set_mode(0o644);
    beta.set_size(nested_body.len() as u64);
    beta.set_cksum();
    builder
        .append_data(
            &mut beta,
            "nested/beta.txt",
            std::io::Cursor::new(nested_body.as_slice()),
        )
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap()
}

#[cfg(unix)]
pub fn directory_tar_with_symlink() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());

    let file_body = b"alpha\n";
    let mut alpha = tar::Header::new_gnu();
    alpha.set_entry_type(tar::EntryType::Regular);
    alpha.set_mode(0o644);
    alpha.set_size(file_body.len() as u64);
    alpha.set_cksum();
    builder
        .append_data(
            &mut alpha,
            "alpha.txt",
            std::io::Cursor::new(file_body.as_slice()),
        )
        .unwrap();

    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Symlink);
    link.set_size(0);
    builder
        .append_link(&mut link, "alpha-link", "alpha.txt")
        .unwrap();

    builder.finish().unwrap();
    builder.into_inner().unwrap()
}
```

Then remove any unused `PathBuf` import if rustfmt/clippy reports it unused. `PathBuf` is included in this step only if the implementation keeps a path-vector helper during editing.

- [ ] **Step 2: Update broker transfer tests to use shared helpers**

In `crates/remote-exec-broker/tests/mcp_transfer.rs`:

1. Remove the local `SINGLE_FILE_ENTRY`, `read_single_file_archive`, and `raw_tar_file_with_path` definitions near the top of the file.
2. Replace the transfer archive import with:

```rust
use support::transfer_archive::{
    SINGLE_FILE_ENTRY, decode_archive, raw_tar_file_with_path, read_archive_paths,
    read_single_file_archive,
};
```

Keep the existing local `std::io::{Cursor, Read}` import only if it is still used after removing the helper definitions. If it is not used, remove it.

- [ ] **Step 3: Update daemon transfer tests to use shared helpers**

In `crates/remote-exec-daemon/tests/transfer_rpc.rs`:

1. Remove the local `raw_tar_file_with_path`, `multi_source_tar`, and Unix-only `directory_tar_with_symlink` definitions.
2. Replace the transfer archive import with:

```rust
use support::transfer_archive::{
    decode_archive, directory_tar_with_symlink, multi_source_tar, raw_tar_file_with_path,
};
```

3. Because `directory_tar_with_symlink` is Unix-only, gate the import exactly as needed if the compiler asks for it:

```rust
use support::transfer_archive::{decode_archive, multi_source_tar, raw_tar_file_with_path};
#[cfg(unix)]
use support::transfer_archive::directory_tar_with_symlink;
```

- [ ] **Step 4: Run focused transfer verification**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture
cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture
cargo fmt --all --check
```

Expected: both transfer suites pass, and formatting is clean.

- [ ] **Step 5: Commit**

```bash
git add \
  tests/support/transfer_archive.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs
git commit -m "test: share transfer archive helpers"
```

### Task 2: Consolidate Broker Transfer Test Calls

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`

**Testing approach:** existing tests + targeted verification.
Reason: This is broker integration test harness cleanup. It should preserve request-shape tests and only simplify repeated ordinary transfer calls.

- [ ] **Step 1: Add small broker transfer helpers**

Add these helpers near the bottom of `crates/remote-exec-broker/tests/mcp_transfer.rs`, before platform path helpers:

```rust
async fn transfer_single_source(
    fixture: &support::fixture::BrokerFixture,
    source_target: &str,
    source_path: impl ToString,
    destination_target: &str,
    destination_path: impl ToString,
    overwrite: Option<&str>,
    create_parent: bool,
) -> support::fixture::ToolResult {
    let mut payload = serde_json::json!({
        "source": {
            "target": source_target,
            "path": source_path.to_string()
        },
        "destination": {
            "target": destination_target,
            "path": destination_path.to_string()
        },
        "create_parent": create_parent
    });
    if let Some(overwrite) = overwrite {
        payload["overwrite"] = serde_json::Value::String(overwrite.to_string());
    }
    fixture.call_tool("transfer_files", payload).await
}

async fn transfer_single_source_error(
    fixture: &support::fixture::BrokerFixture,
    source_target: &str,
    source_path: impl ToString,
    destination_target: &str,
    destination_path: impl ToString,
    overwrite: Option<&str>,
    create_parent: bool,
) -> String {
    let mut payload = serde_json::json!({
        "source": {
            "target": source_target,
            "path": source_path.to_string()
        },
        "destination": {
            "target": destination_target,
            "path": destination_path.to_string()
        },
        "create_parent": create_parent
    });
    if let Some(overwrite) = overwrite {
        payload["overwrite"] = serde_json::Value::String(overwrite.to_string());
    }
    fixture.call_tool_error("transfer_files", payload).await
}
```

- [ ] **Step 2: Convert ordinary single-source calls**

Replace repeated JSON blocks with `transfer_single_source` only where the request is a normal single-source transfer and the exact JSON shape is not the subject of the test.

Good conversion candidates:

- `transfer_files_copies_local_file_and_reports_summary`
- `transfer_files_defaults_to_merge_overwrite`
- `transfer_files_uses_bearer_auth_for_remote_imports`
- `transfer_files_uses_bearer_auth_for_remote_exports`
- `transfer_files_copies_local_file_to_plain_http_remote_as_single_file_tar`
- `transfer_files_auto_negotiates_zstd_when_supported`
- `transfer_files_falls_back_to_none_when_target_does_not_support_compression`
- `transfer_files_copies_plain_http_remote_file_to_local_from_single_file_tar`
- `transfer_files_copies_plain_http_remote_directory_to_local`
- same-local-path and platform path rejection tests that only need ordinary source/destination payloads

Do not convert:

- multi-source `sources` tests
- exclude tests
- destination mode tests
- compression-field rejection tests
- host sandbox tests
- tests where inline JSON makes the public request shape clearer

- [ ] **Step 3: Run focused broker transfer verification**

Run:

```bash
cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture
cargo fmt --all --check
```

Expected: broker transfer tests pass, and formatting is clean.

- [ ] **Step 4: Commit**

```bash
git add crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "test: consolidate broker transfer calls"
```

### Task 3: Consolidate Daemon Transfer RPC Helpers

**Files:**
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`

**Testing approach:** existing tests + targeted verification.
Reason: This task trims repeated daemon transfer RPC setup while keeping transfer behavior cases distinct.

- [ ] **Step 1: Add small daemon transfer helpers**

Add these helpers near the top of `crates/remote-exec-daemon/tests/transfer_rpc.rs`, after imports:

```rust
async fn transfer_path_info(
    fixture: &support::fixture::DaemonFixture,
    path: impl ToString,
) -> reqwest::Response {
    fixture
        .raw_post_json(
            "/v1/transfer/path-info",
            &TransferPathInfoRequest {
                path: path.to_string(),
            },
        )
        .await
}

async fn export_path(
    fixture: &support::fixture::DaemonFixture,
    path: impl ToString,
    compression: TransferCompression,
) -> reqwest::Response {
    fixture
        .raw_post_json(
            "/v1/transfer/export",
            &TransferExportRequest {
                path: path.to_string(),
                compression,
                symlink_mode: Default::default(),
                exclude: Vec::new(),
            },
        )
        .await
}

fn import_headers(
    destination: impl ToString,
    overwrite: &str,
    create_parent: &str,
    source_type: &str,
) -> Vec<(&'static str, String)> {
    vec![
        (TRANSFER_DESTINATION_PATH_HEADER, destination.to_string()),
        (TRANSFER_OVERWRITE_HEADER, overwrite.to_string()),
        (TRANSFER_CREATE_PARENT_HEADER, create_parent.to_string()),
        (TRANSFER_SOURCE_TYPE_HEADER, source_type.to_string()),
    ]
}
```

- [ ] **Step 2: Merge simple path-info success tests**

Replace `transfer_path_info_reports_existing_directory` and `transfer_path_info_reports_missing_destination` with one table-driven test:

```rust
#[tokio::test]
async fn transfer_path_info_reports_existing_directory_and_missing_destination() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let existing = fixture.workdir.join("release");
    tokio::fs::create_dir_all(&existing).await.unwrap();
    let missing = fixture.workdir.join("missing");

    for (path, expected_exists, expected_is_directory) in [
        (existing.display().to_string(), true, true),
        (missing.display().to_string(), false, false),
    ] {
        let response = transfer_path_info(&fixture, path).await;

        assert!(response.status().is_success());
        let info = response.json::<TransferPathInfoResponse>().await.unwrap();
        assert_eq!(info.exists, expected_exists);
        assert_eq!(info.is_directory, expected_is_directory);
    }
}
```

- [ ] **Step 3: Convert repeated export/import setup where it stays clearer**

Use `transfer_path_info`, `export_path`, and `import_headers` for direct repetitions that do not need custom exclude, symlink mode, or missing-header shape.

Good conversion candidates:

- `transfer_path_info_rejects_relative_paths_with_explicit_code`
- `export_file_streams_archive_and_reports_file_source_type`
- `export_file_supports_zstd_compression`
- `export_reports_missing_sources_with_explicit_code`
- import tests that currently build the same four required import headers exactly

Do not convert:

- tests whose purpose is missing or invalid metadata headers
- tests with custom `exclude`
- tests with custom symlink mode
- Windows path normalization tests where inline path construction is the clearest part of the scenario

- [ ] **Step 4: Run focused daemon transfer verification**

Run:

```bash
cargo test -p remote-exec-daemon --test transfer_rpc -- --nocapture
cargo fmt --all --check
```

Expected: daemon transfer tests pass, and formatting is clean.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/tests/transfer_rpc.rs
git commit -m "test: consolidate daemon transfer rpc helpers"
```

### Task 4: Clean Admin And PKI Test Helpers

**Files:**
- Modify: `crates/remote-exec-admin/tests/dev_init.rs`
- Modify: `crates/remote-exec-admin/tests/certs_issue.rs`
- Modify: `crates/remote-exec-pki/tests/ca_reuse.rs`
- Modify: `crates/remote-exec-pki/tests/dev_init_bundle.rs`

**Testing approach:** existing tests + targeted verification.
Reason: These tests are already small and behavior-focused. The useful cleanup is local helper extraction, not trimming behavior.

- [ ] **Step 1: Add admin command helpers in `dev_init.rs`**

At the top of `crates/remote-exec-admin/tests/dev_init.rs`, replace the bare import with these helpers:

```rust
use std::path::Path;
use std::process::{Command, Output};

fn admin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure_contains(output: &Output, expected: &str) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_dev_init(out_dir: &Path, target: &str) -> Output {
    admin()
        .args(["certs", "dev-init", "--out-dir"])
        .arg(out_dir)
        .args(["--target", target])
        .output()
        .expect("dev-init should run")
}
```

Then update repeated `Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))` dev-init calls to use `admin()` or `run_dev_init()`, and update repeated success/failure assertions to use `assert_success` and `assert_failure_contains`.

- [ ] **Step 2: Add admin command helpers in `certs_issue.rs`**

Keep the existing `admin()` helper and add:

```rust
use std::process::Output;

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn init_ca(out_dir: &std::path::Path) {
    let output = admin()
        .args(["certs", "init-ca", "--out-dir"])
        .arg(out_dir)
        .output()
        .expect("init-ca runs");
    assert_success(&output);
}
```

Then replace repeated init-ca command blocks with `init_ca(&ca_dir)` and repeated success assertions with `assert_success(&output)`.

- [ ] **Step 3: Add PEM assertion helpers in PKI tests**

In `crates/remote-exec-pki/tests/ca_reuse.rs`, add:

```rust
fn assert_pem_pair(cert_pem: &str, key_pem: &str) {
    assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(key_pem.contains("BEGIN PRIVATE KEY"));
}
```

Then replace repeated certificate/key PEM assertions in that file.

In `crates/remote-exec-pki/tests/dev_init_bundle.rs`, add:

```rust
fn assert_pem_pair(cert_pem: &str, key_pem: &str) {
    assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(key_pem.contains("BEGIN PRIVATE KEY"));
}
```

Then use it for `bundle.ca`, `bundle.broker`, and `bundle.daemons["builder-a"]`.

- [ ] **Step 4: Run focused admin and PKI verification**

Run:

```bash
cargo test -p remote-exec-admin --test dev_init
cargo test -p remote-exec-admin --test certs_issue
cargo test -p remote-exec-pki --test ca_reuse
cargo test -p remote-exec-pki --test dev_init_bundle
cargo fmt --all --check
```

Expected: admin and PKI tests pass, and formatting is clean.

- [ ] **Step 5: Commit**

```bash
git add \
  crates/remote-exec-admin/tests/dev_init.rs \
  crates/remote-exec-admin/tests/certs_issue.rs \
  crates/remote-exec-pki/tests/ca_reuse.rs \
  crates/remote-exec-pki/tests/dev_init_bundle.rs
git commit -m "test: clean admin and pki test helpers"
```

### Task 5: Clarify Manual Windows PTY Diagnostics

**Files:**
- Modify: `crates/remote-exec-daemon/tests/windows_pty_debug.rs`

**Testing approach:** no new tests needed.
Reason: This task makes existing manual diagnostic intent explicit. The file already contains ignored Windows-only diagnostics and does not change executable behavior.

- [ ] **Step 1: Add module-level diagnostic documentation**

At the top of `crates/remote-exec-daemon/tests/windows_pty_debug.rs`, add:

```rust
//! Manual Windows PTY diagnostics.
//!
//! These tests are intentionally ignored and Windows-only. They are kept under
//! `tests/` so developers can run them with `cargo test -p remote-exec-daemon
//! --test windows_pty_debug -- --ignored --nocapture` while debugging ConPTY or
//! winpty behavior. They are not part of the automated quality gate.
```

If the file already has leading module attributes, place this comment before imports and after any required crate-level attributes.

- [ ] **Step 2: Verify the diagnostic test target still compiles**

Run:

```bash
cargo test -p remote-exec-daemon --test windows_pty_debug
cargo fmt --all --check
```

Expected: the test target compiles. On non-Windows, it should run zero tests or skip Windows-only tests.

- [ ] **Step 3: Commit**

```bash
git add crates/remote-exec-daemon/tests/windows_pty_debug.rs
git commit -m "test: document manual windows pty diagnostics"
```

### Task 6: Clean C++ Route And Session Test Structure

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`

**Testing approach:** existing tests + targeted verification.
Reason: C++ route and session tests are large single-main executables. This task improves navigability by extracting scenario groups while preserving the existing harness and target graph.

- [ ] **Step 1: Refactor `test_server_routes.cpp` into local scenario functions**

In `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`, keep existing helper functions, then split `main()` into these local functions:

```cpp
static void assert_target_info_and_basic_helpers(AppState& state) {
    HttpRequest info_request;
    info_request.method = "POST";
    info_request.path = "/v1/target-info";
    const HttpResponse info_response = route_request(state, info_request);
    assert(info_response.status == 200);
    const Json info = Json::parse(info_response.body);
    assert(info.at("target").get<std::string>() == "cpp-test");
    assert(info.at("supports_pty").get<bool>() == process_session_supports_pty());
    assert(info.at("supports_image_read").get<bool>());
    assert(info.at("supports_port_forward").get<bool>());
    assert(info.at("port_forward_protocol_version").get<int>() == 4);

    assert(normalize_port_forward_endpoint("8080") == "127.0.0.1:8080");
    assert(base64_decode_bytes(base64_encode_bytes(std::string("hello\0world", 11))).size() == 11);
}
```

Then extract the existing contiguous blocks from `main()` into:

```cpp
static void assert_transfer_export_errors(AppState& state, const fs::path& root);
static void assert_image_routes(AppState& state, const fs::path& root);
static void assert_transfer_path_info_routes(AppState& state, const fs::path& root);
static std::string assert_transfer_export_and_exclude_routes(AppState& state, const fs::path& root);
static void assert_transfer_import_routes(AppState& state, const fs::path& root, const std::string& export_body);
static void assert_sandbox_routes(const fs::path& root);
static void assert_exec_routes(AppState& state, const fs::path& root);
```

Move existing statements into these functions without changing assertions. Keep `main()` as orchestration:

```cpp
int main() {
    const fs::path root = make_test_root();
    AppState state;
    initialize_state(state, root);

    assert_target_info_and_basic_helpers(state);
    assert_transfer_export_errors(state, root);
    assert_image_routes(state, root);
    assert_transfer_path_info_routes(state, root);
    const std::string export_body = assert_transfer_export_and_exclude_routes(state, root);
    assert_transfer_import_routes(state, root, export_body);
    assert_sandbox_routes(root);
    assert_exec_routes(state, root);

    return 0;
}
```

Use the actual existing final return style if the file already has one. Do not change request bodies, expected status codes, or assertion text in this step.

- [ ] **Step 2: Refactor `test_session_store.cpp` into local scenario functions**

In `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`, split `main()` into scenario functions without changing assertions:

```cpp
static void assert_completed_command_output(SessionStore& store, const fs::path& root, const std::string& shell, const YieldTimeConfig& yield_time);
static void assert_token_limiting(SessionStore& store, const fs::path& root, const std::string& shell, const YieldTimeConfig& yield_time);
static void assert_posix_locale_and_late_output(SessionStore& store, const fs::path& root, const std::string& shell, const YieldTimeConfig& yield_time);
static void assert_stdin_and_tty_behavior(SessionStore& store, const fs::path& root, const std::string& shell, const YieldTimeConfig& yield_time);
static void assert_pruning_and_recency_behavior(const fs::path& root, const std::string& shell);
static void assert_threshold_warnings_and_unknown_sessions(SessionStore& store, const fs::path& root, const std::string& shell);
```

Keep platform-specific `#ifdef _WIN32` blocks inside the relevant function bodies. `main()` should become a short sequence of these calls.

- [ ] **Step 3: Run focused C++ route/session verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-server-routes
make -C crates/remote-exec-daemon-cpp test-host-session-store
```

Expected: both C++ host test targets pass.

- [ ] **Step 4: Run C++ aggregate verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: C++ POSIX test suite passes.

- [ ] **Step 5: Commit**

```bash
git add \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
  crates/remote-exec-daemon-cpp/tests/test_session_store.cpp
git commit -m "test: organize cpp route and session tests"
```

### Task 7: Clean C++ Streaming Test Structure

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`

**Testing approach:** existing tests + targeted verification.
Reason: `test_server_streaming.cpp` is a large single-main test covering important HTTP streaming and v4 tunnel behavior. This task improves navigability without changing the harness or splitting Makefile targets.

- [ ] **Step 1: Group streaming `main()` scenarios into helper functions**

In `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`, leave socket and frame helpers in place and extract contiguous `main()` scenario blocks into local functions:

```cpp
static void assert_http_streaming_routes(AppState& state, const fs::path& root);
static void assert_tunnel_rejects_invalid_requests(AppState& state);
static void assert_tunnel_open_ready_and_limits(AppState& state);
static void assert_tunnel_tcp_listener_and_connect_paths(AppState& state);
static void assert_tunnel_udp_paths(AppState& state);
static void assert_tunnel_limit_and_pressure_paths(AppState& state);
static void assert_tunnel_resume_and_expiry_paths(AppState& state);
```

Move existing statements into these functions without changing assertion logic. Keep the already-existing specific helpers such as `assert_tunnel_udp_bind_emits_two_peer_datagrams` and `assert_tunnel_tcp_listener_session_can_resume_after_transport_drop`; the new functions should group calls to them and any remaining inline blocks.

Make `main()` read as:

```cpp
int main() {
    NetworkSession network;
    const fs::path root = make_test_root();
    AppState state;
    initialize_state(state, root);

    assert_http_streaming_routes(state, root);
    assert_tunnel_rejects_invalid_requests(state);
    assert_tunnel_open_ready_and_limits(state);
    assert_tunnel_tcp_listener_and_connect_paths(state);
    assert_tunnel_udp_paths(state);
    assert_tunnel_limit_and_pressure_paths(state);
    assert_tunnel_resume_and_expiry_paths(state);

    return 0;
}
```

Use the exact existing initialization and final return style if it differs.

- [ ] **Step 2: Run focused C++ streaming verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: the C++ streaming test target passes.

- [ ] **Step 3: Run C++ aggregate verification**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: C++ POSIX test suite passes.

- [ ] **Step 4: Commit**

```bash
git add crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp
git commit -m "test: organize cpp streaming tests"
```

### Task 8: Final First-Party Test Maintenance Gate

**Files:**
- Verify: first-party Rust workspace and C++ daemon test suite

**Testing approach:** existing tests + full verification.
Reason: This is the final confidence pass after layered test maintenance.

- [ ] **Step 1: Run full Rust workspace tests**

Run:

```bash
cargo test --workspace
```

Expected: the full first-party Rust workspace test suite passes.

- [ ] **Step 2: Run Rust formatting and lint gates**

Run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: formatting and clippy pass with no warnings.

- [ ] **Step 3: Run C++ POSIX gate**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: C++ POSIX daemon tests and build targets pass.

- [ ] **Step 4: Check for whitespace errors and generated tracked changes**

Run:

```bash
git diff --check
git status --short
```

Expected: `git diff --check` exits cleanly. `git status --short` shows no uncommitted tracked changes after all task commits. Ignored generated build output may exist but must not be added.

- [ ] **Step 5: Commit final formatting-only cleanup if needed**

If any formatting-only tracked changes were produced by final verification, inspect them and commit the concrete files. For example, if `cargo fmt` changed Rust test files, run:

```bash
git diff --stat
git add crates/remote-exec-broker/tests/mcp_transfer.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs \
  crates/remote-exec-admin/tests/dev_init.rs \
  crates/remote-exec-admin/tests/certs_issue.rs \
  crates/remote-exec-pki/tests/ca_reuse.rs \
  crates/remote-exec-pki/tests/dev_init_bundle.rs
git commit -m "test: finalize first-party test maintenance"
```

Only add files that actually appear in `git status --short`. If there are no tracked changes, do not create an empty commit.
