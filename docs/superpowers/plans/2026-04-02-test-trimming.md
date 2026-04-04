# Test Trimming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Trim redundant and low-signal tests while preserving the approved broker, daemon image, and admin behavioral coverage.

**Architecture:** This plan changes only test files. The broker test suite is reduced by merging duplicate happy-path assertions and dropping one redundant alias-only interception case, the daemon image suite keeps one meaningful representative per format family by strengthening PNG resize assertions and sharing resize-check logic, and the admin suite drops the shallow CLI help smoke test while retaining functional `dev-init` coverage.

**Tech Stack:** Rust 2024, Tokio, rmcp, cargo test, cargo fmt, clippy

---

## File Map

- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
  - Merge duplicated `exec_command` and `write_stdin` happy-path tests and delete the standalone alias-only broker interception test.
- Modify: `crates/remote-exec-daemon/tests/image_rpc.rs`
  - Add one shared large-image resize helper, strengthen PNG resize assertions, and keep JPEG resize coverage on the same helper.
- Delete: `crates/remote-exec-admin/tests/dev_init_cli.rs`
  - Remove the shallow CLI help smoke test.

### Task 1: Consolidate Broker Exec Coverage

**Files:**
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`

**Testing approach:** `existing tests + targeted verification`
Reason: This task trims duplicate broker integration coverage without changing broker behavior, so the right verification is the existing `mcp_exec` suite.

- [ ] **Step 1: Merge the duplicated `exec_command` happy-path tests into one test**

```bash
apply_patch <<'PATCH'
*** Begin Patch
*** Update File: crates/remote-exec-broker/tests/mcp_exec.rs
@@
-#[tokio::test]
-async fn exec_command_returns_an_opaque_string_session_id() {
-    let fixture = support::spawn_broker_with_stub_daemon().await;
-    let result = fixture
-        .call_tool(
-            "exec_command",
-            serde_json::json!({
-                "target": "builder-a",
-                "cmd": "printf ready; sleep 2",
-                "tty": true,
-                "yield_time_ms": 250
-            }),
-        )
-        .await;
-
-    let session_id = result.structured_content["session_id"]
-        .as_str()
-        .expect("running session");
-    assert!(session_id.starts_with("sess_"));
-    assert!(result.structured_content["exit_code"].is_null());
-}
-
-#[tokio::test]
-async fn exec_command_structured_output_includes_session_command() {
+#[tokio::test]
+async fn exec_command_returns_opaque_session_id_and_session_command() {
     let fixture = support::spawn_broker_with_stub_daemon().await;
     let result = fixture
         .call_tool(
             "exec_command",
             serde_json::json!({
@@
         )
         .await;
 
+    let session_id = result.structured_content["session_id"]
+        .as_str()
+        .expect("running session");
+    assert!(session_id.starts_with("sess_"));
+    assert!(result.structured_content["exit_code"].is_null());
     assert_eq!(
         result.structured_content["session_command"],
         serde_json::Value::String("printf ready; sleep 2".to_string())
     );
 }
*** End Patch
PATCH
```

- [ ] **Step 2: Run focused verification for the merged `exec_command` test**

Run: `cargo test -p remote-exec-broker exec_command_returns_opaque_session_id_and_session_command -- --nocapture`
Expected: PASS with the merged broker test proving both the opaque public session id and the preserved `session_command`.

- [ ] **Step 3: Merge the duplicated `write_stdin` happy-path tests and delete the alias-only broker interception test**

```bash
apply_patch <<'PATCH'
*** Begin Patch
*** Update File: crates/remote-exec-broker/tests/mcp_exec.rs
@@
-#[tokio::test]
-async fn exec_command_intercepts_applypatch_alias_without_allocating_session() {
-    let fixture = support::spawn_broker_with_stub_daemon().await;
-    let patch = concat!(
-        "*** Begin Patch\n",
-        "*** Add File: alias.txt\n",
-        "+alias\n",
-        "*** End Patch\n",
-    );
-
-    let result = fixture
-        .call_tool(
-            "exec_command",
-            serde_json::json!({
-                "target": "builder-a",
-                "cmd": format!("applypatch \"{patch}\""),
-            }),
-        )
-        .await;
-
-    assert!(result.structured_content["session_id"].is_null());
-    assert_eq!(fixture.exec_start_calls().await, 0);
-    assert_eq!(
-        fixture.last_patch_request().await.unwrap().patch,
-        patch.to_string()
-    );
-}
@@
-#[tokio::test]
-async fn write_stdin_routes_by_public_session_id_instead_of_target_guessing() {
+#[tokio::test]
+async fn write_stdin_routes_by_public_session_id_and_preserves_original_command_metadata() {
     let fixture = support::spawn_broker_with_stub_daemon().await;
     let started = fixture
         .call_tool(
             "exec_command",
             serde_json::json!({
@@
     assert!(
         result.structured_content["output"]
             .as_str()
             .unwrap()
             .contains("poll output")
     );
-}
-
-#[tokio::test]
-async fn write_stdin_preserves_original_command_metadata() {
-    let fixture = support::spawn_broker_with_stub_daemon().await;
-    let started = fixture
-        .call_tool(
-            "exec_command",
-            serde_json::json!({
-                "target": "builder-a",
-                "cmd": "printf ready; sleep 2",
-                "tty": true,
-                "yield_time_ms": 250
-            }),
-        )
-        .await;
-    let session_id = started.structured_content["session_id"]
-        .as_str()
-        .expect("running session")
-        .to_string();
-
-    let result = fixture
-        .call_tool(
-            "write_stdin",
-            serde_json::json!({
-                "session_id": session_id,
-                "chars": "",
-                "yield_time_ms": 5000
-            }),
-        )
-        .await;
 
     assert!(
         result
             .text_output
             .contains("Command: printf ready; sleep 2")
*** End Patch
PATCH
```

- [ ] **Step 4: Run post-change broker verification**

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: PASS with the merged `exec_command` and `write_stdin` tests present, the alias-only test absent, and all other broker exec coverage unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/tests/mcp_exec.rs
git commit -m "test: trim broker exec coverage overlap"
```

### Task 2: Strengthen Image Resize Coverage

**Files:**
- Modify: `crates/remote-exec-daemon/tests/image_rpc.rs`
- Test/Verify: `cargo test -p remote-exec-daemon --test image_rpc -- --nocapture`
- Test/Verify: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`

**Testing approach:** `existing tests + targeted verification`
Reason: This task keeps the same image behavior coverage but makes the PNG resize test meaningful and slightly reduces maintenance by sharing resize assertions.

- [ ] **Step 1: Add one shared large-image resize helper and strengthen PNG resize coverage**

```bash
apply_patch <<'PATCH'
*** Begin Patch
*** Update File: crates/remote-exec-daemon/tests/image_rpc.rs
@@
 async fn assert_default_passthrough(extension: &str, format: ImageFormat, expected_mime: &str) {
@@
     assert_eq!(response.detail, None);
 }
+
+async fn assert_resized_output(extension: &str, format: ImageFormat, expected_mime: &str) {
+    let fixture = support::spawn_daemon("builder-a").await;
+    let path = fixture.workdir.join(format!("large.{extension}"));
+    support::write_image(&path, 4096, 2048, format).await;
+
+    let response = fixture
+        .rpc::<ImageReadRequest, ImageReadResponse>(
+            "/v1/image/read",
+            &ImageReadRequest {
+                path: format!("large.{extension}"),
+                workdir: Some(".".to_string()),
+                detail: None,
+            },
+        )
+        .await;
+
+    let (mime, bytes) = support::decode_data_url(&response.image_url);
+    let image = image::load_from_memory(&bytes).unwrap();
+    assert_eq!(mime, expected_mime);
+    assert!(image.width() <= 2048);
+    assert!(image.height() <= 768);
+    assert_eq!(response.detail, None);
+}
@@
-#[tokio::test]
-async fn image_read_resizes_large_images_by_default() {
-    let fixture = support::spawn_daemon("builder-a").await;
-    let path = fixture.workdir.join("large.png");
-    support::write_png(&path, 4096, 2048).await;
-
-    let response = fixture
-        .rpc::<ImageReadRequest, ImageReadResponse>(
-            "/v1/image/read",
-            &ImageReadRequest {
-                path: "large.png".to_string(),
-                workdir: Some(".".to_string()),
-                detail: None,
-            },
-        )
-        .await;
-
-    assert!(response.image_url.starts_with("data:image/png;base64,"));
-    assert_eq!(response.detail, None);
-}
+#[tokio::test]
+async fn image_read_resizes_large_png_and_keeps_png_encoding() {
+    assert_resized_output("png", ImageFormat::Png, "image/png").await;
+}
*** End Patch
PATCH
```

- [ ] **Step 2: Run focused verification for the strengthened PNG resize test**

Run: `cargo test -p remote-exec-daemon image_read_resizes_large_png_and_keeps_png_encoding -- --nocapture`
Expected: PASS with the PNG resize test decoding the returned image, asserting `image/png`, and proving the `2048x768` default resize bounds.

- [ ] **Step 3: Switch the JPEG resize test to the same helper so the resize-family assertions live in one place**

```bash
apply_patch <<'PATCH'
*** Begin Patch
*** Update File: crates/remote-exec-daemon/tests/image_rpc.rs
@@
 #[tokio::test]
 async fn image_read_resizes_large_jpeg_and_keeps_jpeg_encoding() {
-    let fixture = support::spawn_daemon("builder-a").await;
-    let path = fixture.workdir.join("large.jpg");
-    support::write_image(&path, 4096, 2048, ImageFormat::Jpeg).await;
-
-    let response = fixture
-        .rpc::<ImageReadRequest, ImageReadResponse>(
-            "/v1/image/read",
-            &ImageReadRequest {
-                path: "large.jpg".to_string(),
-                workdir: Some(".".to_string()),
-                detail: None,
-            },
-        )
-        .await;
-
-    let (mime, bytes) = support::decode_data_url(&response.image_url);
-    let image = image::load_from_memory(&bytes).unwrap();
-    assert_eq!(mime, "image/jpeg");
-    assert!(image.width() <= 2048);
-    assert!(image.height() <= 768);
+    assert_resized_output("jpg", ImageFormat::Jpeg, "image/jpeg").await;
 }
*** End Patch
PATCH
```

- [ ] **Step 4: Run post-change image verification**

Run: `cargo test -p remote-exec-daemon --test image_rpc -- --nocapture`
Expected: PASS with passthrough, PNG resize, JPEG resize, GIF re-encode, and error-surface coverage all green.

Run: `cargo test -p remote-exec-broker --test mcp_assets -- --nocapture`
Expected: PASS with the broker asset-facing surface unchanged after the daemon image test cleanup.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-daemon/tests/image_rpc.rs
git commit -m "test: strengthen image resize coverage"
```

### Task 3: Remove Shallow Admin Help Coverage And Re-verify The Workspace

**Files:**
- Delete: `crates/remote-exec-admin/tests/dev_init_cli.rs`
- Test/Verify: `cargo test -p remote-exec-admin --test dev_init -- --nocapture`
- Test/Verify: `cargo test --workspace -- --list`
- Test/Verify: `cargo test --workspace`
- Test/Verify: `cargo fmt --all --check`
- Test/Verify: `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: This task removes a low-value test file and then proves the trimmed suite still covers the intended behavior through the remaining functional admin test and the full workspace gate.

- [ ] **Step 1: Remove the CLI help smoke test file**

```bash
apply_patch <<'PATCH'
*** Begin Patch
*** Delete File: crates/remote-exec-admin/tests/dev_init_cli.rs
*** End Patch
PATCH
```

- [ ] **Step 2: Run focused admin verification**

Run: `cargo test -p remote-exec-admin --test dev_init -- --nocapture`
Expected: PASS with the remaining functional `dev-init` tests green.

- [ ] **Step 3: Confirm the trimmed test surface through the workspace test list**

Run: `cargo test --workspace -- --list`
Expected: the removed names are absent:
- `dev_init_help_lists_required_flags`
- `exec_command_intercepts_applypatch_alias_without_allocating_session`
- `exec_command_structured_output_includes_session_command`
- `write_stdin_preserves_original_command_metadata`
- `image_read_resizes_large_images_by_default`

Expected: the merged or strengthened names are present:
- `exec_command_returns_opaque_session_id_and_session_command`
- `write_stdin_routes_by_public_session_id_and_preserves_original_command_metadata`
- `image_read_resizes_large_png_and_keeps_png_encoding`

- [ ] **Step 4: Run the full quality gate**

Run: `cargo test --workspace`
Expected: PASS across all workspace crates and end-to-end tests.

Run: `cargo fmt --all --check`
Expected: PASS with no formatting diff.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS with zero clippy warnings.

- [ ] **Step 5: Commit**

```bash
git add -u crates/remote-exec-admin/tests/dev_init_cli.rs
git commit -m "test: drop shallow admin help coverage"
```
