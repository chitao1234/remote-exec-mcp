# Windows XP Folder Transfer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cross-target directory transfer support to the standalone Windows XP daemon without changing the public broker transfer contract.

**Architecture:** Keep the broker transport and public tool schema unchanged, then teach `remote-exec-daemon-xp` to export directories as GNU tar streams and import the same narrow GNU tar subset. Cover the new behavior at the XP transfer seam, through broker public tests, and with a real Wine-backed verification run against the built XP daemon.

**Tech Stack:** Rust 2024 workspace, C++17 XP daemon, GNU tar-compatible archive encoding, Axum/reqwest broker tests, Wine, `i686-w64-mingw32-g++`

---

### Task 1: Lock The XP Directory Transfer Contract With Failing Tests

**Files:**
- Modify: `crates/remote-exec-daemon-xp/tests/test_transfer.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-xp test-host-transfer`

**Testing approach:** `TDD`
Reason: the behavior seam is clean in `transfer_ops.cpp`, and failing tests will pin the exact directory semantics before the archive code is written.

- [ ] **Step 1: Add failing XP transfer tests for directory export/import, empty directories, overwrite behavior, and traversal rejection**

```cpp
// crates/remote-exec-daemon-xp/tests/test_transfer.cpp
static void assert_directory_round_trip() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-dir";
    fs::remove_all(root);
    fs::create_directories(root / "source" / "nested" / "empty");
    write_text(root / "source" / "nested" / "hello.txt", "hello directory");
    write_text(root / "source" / "top.txt", "top level");

    const ExportedPayload exported = export_path((root / "source").string());
    assert(exported.source_type == "directory");

    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "dest").string(),
        true,
        true
    );

    assert(imported.source_type == "directory");
    assert(imported.files_copied == 2);
    assert(imported.directories_copied >= 3);
    assert(read_text(root / "dest" / "nested" / "hello.txt") == "hello directory");
    assert(read_text(root / "dest" / "top.txt") == "top level");
    assert(fs::is_directory(root / "dest" / "nested" / "empty"));
}

static void assert_directory_replace_behavior() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-replace";
    fs::remove_all(root);
    fs::create_directories(root / "source");
    fs::create_directories(root / "dest" / "stale");
    write_text(root / "source" / "fresh.txt", "fresh");
    write_text(root / "dest" / "stale" / "old.txt", "old");

    const ExportedPayload exported = export_path((root / "source").string());
    const ImportSummary imported = import_path(
        exported.bytes,
        exported.source_type,
        (root / "dest").string(),
        true,
        true
    );

    assert(imported.replaced);
    assert(!fs::exists(root / "dest" / "stale" / "old.txt"));
    assert(read_text(root / "dest" / "fresh.txt") == "fresh");
}

static void assert_directory_traversal_is_rejected() {
    const std::string archive = tar_with_single_file("../escape.txt", "bad");
    const fs::path root = fs::temp_directory_path() / "remote-exec-xp-transfer-traversal";
    fs::remove_all(root);
    bool rejected = false;
    try {
        (void)import_path(archive, "directory", (root / "dest").string(), true, true);
    } catch (...) {
        rejected = true;
    }
    assert(rejected);
}

int main() {
    // Keep the existing file-path tests, then add:
    assert_directory_round_trip();
    assert_directory_replace_behavior();
    assert_directory_traversal_is_rejected();
    return 0;
}
```

- [ ] **Step 2: Run the focused verification and confirm the new directory tests fail against the single-file-only implementation**

Run: `make -C crates/remote-exec-daemon-xp test-host-transfer`
Expected: FAIL because the generalized `export_path` / `import_path` API does not exist yet and the current implementation only supports raw single-file transfer.

- [ ] **Step 3: Add minimal test helpers for tar fixtures so the import tests do not depend on the production archive writer**

```cpp
// crates/remote-exec-daemon-xp/tests/test_transfer.cpp
static std::string octal_field(std::size_t width, std::uint64_t value);
static void append_tar_file(std::string& archive, const std::string& path, const std::string& body);
static void append_tar_directory(std::string& archive, const std::string& path);
static void finalize_tar(std::string& archive);
static std::string tar_with_single_file(const std::string& path, const std::string& body);
```

- [ ] **Step 4: Re-run the focused verification to keep the test fixture compileable before the transfer implementation lands**

Run: `make -C crates/remote-exec-daemon-xp test-host-transfer`
Expected: FAIL only because the generalized transfer API and directory behavior are still missing, not because the tar test fixture code is malformed.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-xp/tests/test_transfer.cpp
git commit -m "test: add xp directory transfer coverage"
```

### Task 2: Implement Narrow GNU Tar Directory Transfer In The XP Daemon

**Files:**
- Modify: `crates/remote-exec-daemon-xp/include/transfer_ops.h`
- Modify: `crates/remote-exec-daemon-xp/src/transfer_ops.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-xp test-host-transfer`

**Testing approach:** `TDD`
Reason: Task 1 defines the expected semantics already, so this task should only add the code required to make those tests pass.

- [ ] **Step 1: Replace the file-only transfer API with a generalized payload API**

```cpp
// crates/remote-exec-daemon-xp/include/transfer_ops.h
struct ExportedPayload {
    std::string source_type;
    std::string bytes;
};

ExportedPayload export_path(const std::string& absolute_path);
ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
);
```

- [ ] **Step 2: Implement tar export/import helpers, destination replacement, and strict entry validation**

```cpp
// crates/remote-exec-daemon-xp/src/transfer_ops.cpp
namespace {
struct TarHeaderView {
    std::string path;
    char typeflag;
    std::uint64_t size;
};

ExportedPayload export_directory_as_tar(const std::string& absolute_path);
ImportSummary import_directory_from_tar(
    const std::string& archive,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
);
TarHeaderView parse_header(const char* block);
std::string read_gnu_long_name(const std::string& archive, std::size_t* offset, std::uint64_t size);
std::string validate_relative_archive_path(const std::string& raw_path);
void append_directory_entry(std::string* archive, const std::string& rel_path);
void append_file_entry(std::string* archive, const std::string& rel_path, const std::string& body);
bool is_zero_block(const char* block);
void remove_existing_path(const std::string& absolute_path);
}

ExportedPayload export_path(const std::string& absolute_path) {
    if (is_regular_file(absolute_path)) {
        return ExportedPayload{"file", read_binary_file(absolute_path)};
    }
    if (is_directory(absolute_path)) {
        return export_directory_as_tar(absolute_path);
    }
    throw std::runtime_error("transfer source must be a regular file or directory");
}

ImportSummary import_path(
    const std::string& bytes,
    const std::string& source_type,
    const std::string& absolute_path,
    bool replace_existing,
    bool create_parent
) {
    if (source_type == "file") {
        return import_file(bytes, absolute_path, replace_existing, create_parent);
    }
    if (source_type == "directory") {
        return import_directory_from_tar(bytes, absolute_path, replace_existing, create_parent);
    }
    throw std::runtime_error("unsupported transfer source type");
}
```

- [ ] **Step 3: Run the focused verification for the helper seam**

Run: `make -C crates/remote-exec-daemon-xp test-host-transfer`
Expected: PASS, with file transfer still working and the new directory tests green.

- [ ] **Step 4: Build the XP executable to confirm the transfer code still cross-compiles with the MinGW toolchain**

Run: `make -C crates/remote-exec-daemon-xp all`
Expected: PASS, producing `crates/remote-exec-daemon-xp/build/remote-exec-daemon-xp.exe`.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-xp/include/transfer_ops.h \
  crates/remote-exec-daemon-xp/src/transfer_ops.cpp
git commit -m "feat: add xp directory transfer support"
```

### Task 3: Wire The HTTP Surface And Add Broker Public Coverage

**Files:**
- Modify: `crates/remote-exec-daemon-xp/src/server.cpp`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`

**Testing approach:** `TDD`
Reason: the broker public surface is already stable; a failing end-to-end style test through the broker is the safest way to prove the XP daemon contract was preserved.

- [ ] **Step 1: Add failing broker tests for remote directory export and import through a plain HTTP XP-style stub**

```rust
// crates/remote-exec-broker/tests/mcp_transfer.rs
#[tokio::test]
async fn transfer_files_copies_local_directory_to_plain_http_remote() {
    let fixture = support::spawn_broker_with_plain_http_stub_daemon().await;
    let source = fixture._tempdir.path().join("source");
    std::fs::create_dir_all(source.join("nested/empty")).unwrap();
    std::fs::write(source.join("nested/hello.txt"), "hello remote\n").unwrap();

    let result = fixture
        .call_tool(
            "transfer_files",
            serde_json::json!({
                "source": { "target": "local", "path": source.display().to_string() },
                "destination": { "target": "builder-xp", "path": "C:/dest/tree" },
                "overwrite": "replace",
                "create_parent": true
            }),
        )
        .await;

    assert_eq!(result.structured_content["source_type"], "directory");
    assert_eq!(result.structured_content["files_copied"], 1);
    assert!(fixture.last_transfer_import().await.unwrap().body_len > 0);
    assert_eq!(
        fixture.last_transfer_import().await.unwrap().source_type,
        "directory"
    );
}
```

- [ ] **Step 2: Extend the plain HTTP stub daemon with transfer endpoints and capture state**

```rust
// crates/remote-exec-broker/tests/support/mod.rs
#[derive(Debug, Clone)]
pub struct StubTransferImportCapture {
    pub destination_path: String,
    pub source_type: String,
    pub overwrite: String,
    pub create_parent: String,
    pub body_len: usize,
}

async fn transfer_export(
    State(state): State<StubDaemonState>,
    Json(req): Json<remote_exec_proto::rpc::TransferExportRequest>,
) -> Result<(axum::http::HeaderMap, Vec<u8>), (StatusCode, Json<RpcErrorBody>)>;

async fn transfer_import(
    State(state): State<StubDaemonState>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<remote_exec_proto::rpc::TransferImportResponse>, (StatusCode, Json<RpcErrorBody>)>;
```

- [ ] **Step 3: Update the XP daemon HTTP handlers to call the generalized transfer API for both files and directories**

```cpp
// crates/remote-exec-daemon-xp/src/server.cpp
if (request.path == "/v1/transfer/export") {
    const Json body = parse_json_body(request);
    const ExportedPayload payload = export_path(body.at("path").get<std::string>());
    response.headers["Content-Type"] = "application/octet-stream";
    response.headers["x-remote-exec-source-type"] = payload.source_type;
    response.body = payload.bytes;
    return response;
}

if (request.path == "/v1/transfer/import") {
    const std::string source_type = request.header("x-remote-exec-source-type");
    const ImportSummary summary = import_path(
        request.body,
        source_type,
        request.header("x-remote-exec-destination-path"),
        request.header("x-remote-exec-overwrite") == "replace",
        request.header("x-remote-exec-create-parent") == "true"
    );
    // serialize summary as before
}
```

- [ ] **Step 4: Run the broker coverage**

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: PASS, including the new directory transfer coverage through the public `transfer_files` tool.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-xp/src/server.cpp \
  crates/remote-exec-broker/tests/support/mod.rs \
  crates/remote-exec-broker/tests/mcp_transfer.rs
git commit -m "test: cover xp directory transfer through broker"
```

### Task 4: Update Docs And Prove The Real Wine Path

**Files:**
- Modify: `crates/remote-exec-daemon-xp/README.md`
- Modify: `README.md`
- Test/Verify: `make -C crates/remote-exec-daemon-xp check`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
- Test/Verify: `cargo test --workspace`
- Test/Verify: Wine-backed broker plus XP daemon directory transfer run

**Testing approach:** `existing tests + targeted verification`
Reason: this task is mostly documentation plus final integration proof; the behavioral seams are already covered by Tasks 1-3.

- [ ] **Step 1: Update the docs to replace the single-file-only XP transfer limitation with the new directory support contract**

```md
<!-- crates/remote-exec-daemon-xp/README.md -->
- `transfer_files` supports regular files and directory trees via the existing broker transfer contract.
- Directory transfers use GNU tar payloads for cross-target compatibility.
- Unsupported archive entries remain rejected on XP: symlinks, hard links, special files, sparse entries, and malformed paths.

<!-- README.md -->
- Windows XP targets now support `transfer_files` for both files and directories when `allow_insecure_http = true` is set on the broker target.
```

- [ ] **Step 2: Run the focused project verification before the Wine check**

Run: `make -C crates/remote-exec-daemon-xp check`
Expected: PASS

Run: `cargo test -p remote-exec-broker --test mcp_transfer -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run the real Wine-backed directory transfer verification**

```bash
make -C crates/remote-exec-daemon-xp all
wine crates/remote-exec-daemon-xp/build/remote-exec-daemon-xp.exe \
  crates/remote-exec-daemon-xp/config/daemon-xp.example.ini &
XP_PID=$!
trap 'kill "$XP_PID"; wait "$XP_PID" 2>/dev/null || true' EXIT

ROOT="$(mktemp -d)"
mkdir -p "$ROOT/source/nested/empty" "$ROOT/exported"
printf 'hello xp directory\n' > "$ROOT/source/nested/hello.txt"
tar --format=gnu -cf "$ROOT/source.tar" -C "$ROOT/source" .

curl -sS -X POST \
  -H 'x-remote-exec-source-type: directory' \
  -H 'x-remote-exec-destination-path: C:/remote-exec/tree' \
  -H 'x-remote-exec-overwrite: replace' \
  -H 'x-remote-exec-create-parent: true' \
  --data-binary @"$ROOT/source.tar" \
  http://127.0.0.1:7878/v1/transfer/import > "$ROOT/import.json"

curl -sS -D "$ROOT/export.headers" \
  -H 'content-type: application/json' \
  -X POST \
  --data '{"path":"C:/remote-exec/tree"}' \
  http://127.0.0.1:7878/v1/transfer/export > "$ROOT/export.tar"

grep -qi '^x-remote-exec-source-type: directory' "$ROOT/export.headers"
tar -xf "$ROOT/export.tar" -C "$ROOT/exported"
test -f "$ROOT/exported/nested/hello.txt"
test -d "$ROOT/exported/nested/empty"
grep -q 'hello xp directory' "$ROOT/exported/nested/hello.txt"
```

Expected: PASS, proving that a Linux-generated GNU tar directory archive imports on the Wine-hosted XP daemon and exports back as a directory tar that Linux can unpack with the same file and empty-directory structure intact.

- [ ] **Step 4: Run the workspace quality gate**

Run: `cargo test --workspace`
Expected: PASS

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon-xp/README.md README.md \
  docs/superpowers/specs/2026-04-04-xp-folder-transfer-design.md \
  docs/superpowers/plans/2026-04-04-xp-folder-transfer.md
git commit -m "docs: describe xp directory transfer support"
```
