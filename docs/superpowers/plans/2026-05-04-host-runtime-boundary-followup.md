# Host Runtime Boundary Follow-Up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Remove the remaining image/transfer transport leakage from `remote-exec-host`, classify request-shape failures and internal filesystem faults correctly, and lock the behavior down with regression coverage.

**Architecture:** Keep `remote-exec-host` focused on local runtime operations plus typed host errors, not Axum handlers or HTTP header parsing for image/transfer. Let `remote-exec-daemon` own transfer import header validation, HTTP body adaptation, and HTTP response shaping, while `remote-exec-host` stays on transport-neutral request types and `AsyncRead` seams. Both Rust and C++ daemons should distinguish request faults from true internal filesystem failures on image and transfer routes.

**Tech Stack:** Rust 2024, Tokio, axum, reqwest, serde, existing daemon integration fixtures, C++17 route harnesses, cargo test, cargo fmt, cargo clippy, make

---

### Task 1: Add the red regression coverage first

**Files:**
- Create/Modify: `docs/superpowers/plans/2026-05-04-host-runtime-boundary-followup.md`
- Modify: `crates/remote-exec-daemon/tests/image_rpc.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Test/Verify: `cargo test -p remote-exec-daemon --test image_rpc image_read_reports_permission_denied_as_internal_error`, `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_missing_destination_header_as_bad_request`, `cargo test -p remote-exec-daemon --test transfer_rpc transfer_path_info_reports_permission_denied_as_internal_error`, `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `TDD`
Reason: the task changes observable RPC codes/status for concrete route behavior, and each behavior has a direct automated test seam.

- [ ] **Step 1: Add failing Rust and C++ route tests for the desired public behavior**

```rust
// crates/remote-exec-daemon/tests/image_rpc.rs
// Add a Unix-only test that creates a directory with execute/search permissions removed,
// requests `/v1/image/read` for a file underneath it, and asserts `500/internal_error`.

// crates/remote-exec-daemon/tests/transfer_rpc.rs
// Add one test that omits `x-remote-exec-destination-path` on `/v1/transfer/import`
// and asserts `400/bad_request`.
// Add one Unix-only test that calls `/v1/transfer/path-info` on a child under a blocked
// directory and asserts `500/internal_error`.
```

```cpp
// crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
// Add a POSIX-only blocked-directory image/read assertion for `500/internal_error`.
// Add a POSIX-only blocked-directory transfer/path-info assertion for `500/internal_error`.
```

- [ ] **Step 2: Run the new focused tests and confirm they fail for the expected reason**

Run: `cargo test -p remote-exec-daemon --test image_rpc image_read_reports_permission_denied_as_internal_error`
Expected: FAIL because the route still reports a client error for the blocked-path filesystem fault.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_missing_destination_header_as_bad_request`
Expected: FAIL because the route still reports `transfer_failed` or another non-`bad_request` code.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc transfer_path_info_reports_permission_denied_as_internal_error`
Expected: PASS if transfer already behaves correctly; keep it as regression coverage either way.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: FAIL because the blocked image/read path is still classified as `image_missing` or `image_decode_failed`.

- [ ] **Step 3: Commit the red coverage only if it is isolated and useful; otherwise keep it in the working tree for the implementation step**

```bash
git add crates/remote-exec-daemon/tests/image_rpc.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp \
  docs/superpowers/plans/2026-05-04-host-runtime-boundary-followup.md
git status --short
```

- [ ] **Step 4: Do not claim success yet; move directly to the minimal implementation**

```text
Red tests are expected here. The point is to prove the seam before editing production code.
```

### Task 2: Remove the remaining host transport leakage and fix the classifications

**Files:**
- Modify: `crates/remote-exec-host/src/image.rs`
- Modify: `crates/remote-exec-host/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/src/image.rs`
- Modify: `crates/remote-exec-daemon/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Test/Verify: `cargo test -p remote-exec-daemon --test image_rpc image_read_reports_permission_denied_as_internal_error`, `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_missing_destination_header_as_bad_request`, `cargo test -p remote-exec-daemon --test transfer_rpc transfer_path_info_reports_permission_denied_as_internal_error`, `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

**Testing approach:** `TDD`
Reason: the implementation should be the smallest change set that turns the red route tests green while shrinking the transport boundary.

- [ ] **Step 1: Remove unused Axum image/transfer handlers from `remote-exec-host` and keep only local-runtime functions there**

```rust
// crates/remote-exec-host/src/image.rs
// Delete `read_image(...)` and the local `host_rpc_error_response(...)` helper.
// Keep `read_image_local(...)` plus pure helpers.

// crates/remote-exec-host/src/transfer/mod.rs
// Delete `path_info(...)`, `export_path(...)`, `import_archive(...)`,
// `parse_import_request(...)`, header parsing helpers, `map_transfer_error(...)`,
// and the local `host_rpc_error_response(...)` helper.
// Keep `path_info_for_request(...)`, `export_path_local(...)`, `import_archive_local(...)`,
// `classify_transfer_error(...)`, and archive helpers.
```

- [ ] **Step 2: Move import-header validation into the daemon transport and classify malformed/missing headers as `bad_request`**

```rust
// crates/remote-exec-daemon/src/transfer/mod.rs
// Add a local `parse_import_request(headers: &HeaderMap)` helper that returns
// `Result<TransferImportRequest, (StatusCode, Json<RpcErrorBody>)>`.
// Use `crate::exec::rpc_error("bad_request", ...)` for missing/invalid headers.
// Keep host-error mapping only for `TransferError -> HostRpcError -> HTTP`.
```

- [ ] **Step 3: Reclassify Rust image filesystem faults so only real decode failures stay `image_decode_failed`**

```rust
// crates/remote-exec-host/src/image.rs
// Change metadata handling to:
// - `NotFound` => `ImageError::missing(...)`
// - other metadata errors => `ImageError::internal(...)`
// Change raw file read failures and impossible output-format / encode failures to
// `ImageError::internal(...)`.
// Keep invalid image bytes / unsupported input image formats as `ImageError::decode_failed(...)`.
```

- [ ] **Step 4: Reclassify C++ image filesystem faults the same way**

```cpp
// crates/remote-exec-daemon-cpp/src/server_routes.cpp
// Replace the `image_path_exists` / `image_path_is_regular_file` helpers with a single
// stat-based helper path that:
// - returns `Missing` only for ENOENT-like cases
// - returns `NotFile` for existing non-regular files
// - throws `ImageFailure(ImageRpcCode::Internal, ...)` for permission or other stat/open failures
// Update file-read failures to surface `Internal` instead of `DecodeFailed`.
```

- [ ] **Step 5: Run the focused post-change verification and confirm the exact public codes/statuses**

Run: `cargo test -p remote-exec-daemon --test image_rpc image_read_reports_permission_denied_as_internal_error`
Expected: PASS with `500/internal_error`.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc import_rejects_missing_destination_header_as_bad_request`
Expected: PASS with `400/bad_request`.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc transfer_path_info_reports_permission_denied_as_internal_error`
Expected: PASS with `500/internal_error`.

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS with the new blocked-path image and transfer assertions green.

- [ ] **Step 6: Commit the implementation slice**

```bash
git add docs/superpowers/plans/2026-05-04-host-runtime-boundary-followup.md \
  crates/remote-exec-host/src/image.rs \
  crates/remote-exec-host/src/transfer/mod.rs \
  crates/remote-exec-daemon/src/image.rs \
  crates/remote-exec-daemon/src/transfer/mod.rs \
  crates/remote-exec-daemon/tests/image_rpc.rs \
  crates/remote-exec-daemon/tests/transfer_rpc.rs \
  crates/remote-exec-daemon-cpp/src/server_routes.cpp \
  crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp
git commit -m "refactor: tighten image and transfer boundary handling"
```

### Task 3: Run the full required verification for the touched surface

**Files:**
- Modify: none if the gate passes
- Test/Verify: `cargo test -p remote-exec-daemon --test image_rpc`, `cargo test -p remote-exec-daemon --test transfer_rpc`, `make -C crates/remote-exec-daemon-cpp test-host-server-routes`, `make -C crates/remote-exec-daemon-cpp check-posix`, `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: after the red-green loop, the touched crates need the repo’s stated quality gate before completion and before any final success claim.

- [ ] **Step 1: Run the focused Rust daemon suites**

Run: `cargo test -p remote-exec-daemon --test image_rpc`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: PASS.

- [ ] **Step 2: Run the focused C++ daemon suites**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
Expected: PASS.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: PASS.

- [ ] **Step 3: Run the final workspace gate**

Run: `cargo test --workspace`
Expected: PASS.

Run: `cargo fmt --all --check`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS.

- [ ] **Step 4: If any gate fails, fix only the reported issue and re-run that gate before moving on**

```text
Do not guess. Re-run the exact failing command until the output is clean.
```
