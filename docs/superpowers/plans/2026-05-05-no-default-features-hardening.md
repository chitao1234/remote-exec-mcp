# No-Default-Features Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Add explicit CI and direct tests for the broker and daemon `tls-disabled` / no-default-features configurations so feature-off behavior is intentionally verified rather than covered only indirectly.

**Architecture:** Keep the current feature split intact and harden it in three layers: direct unit coverage for disabled-path helpers, crate-level no-default-features test and clippy lanes in CI, and README quality-gate documentation for the new coverage. Reuse the existing HTTP-only broker and daemon tests instead of introducing a second transport-specific test harness.

**Tech Stack:** Rust workspace tests, Cargo feature flags, GitHub Actions, existing broker and daemon unit/integration test harnesses.

---

### Task 1: Add direct disabled-path tests for broker and daemon TLS helpers

**Files:**
- Modify: `crates/remote-exec-broker/src/broker_tls.rs`
- Modify: `crates/remote-exec-daemon/src/tls.rs`
- Test/Verify: `cargo test -p remote-exec-broker --no-default-features --lib --locked`
- Test/Verify: `cargo test -p remote-exec-daemon --no-default-features --lib --locked`

**Testing approach:** `TDD`
Reason: The feature-disabled helper behavior is small, deterministic, and already split behind cfg-selected modules, so direct failing tests are the cleanest way to lock the contract down.

- [ ] **Step 1: Add failing broker and daemon disabled-path unit tests**

```rust
// crates/remote-exec-broker/src/broker_tls.rs
    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn build_daemon_https_client_is_rejected_when_feature_disabled() {
        let config: crate::config::TargetConfig = toml::from_str(
            r#"
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/client.pem"
client_key_pem = "/tmp/client.key"
"#,
        )
        .unwrap();

        let err = super::build_daemon_https_client(&config).await.unwrap_err();
        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "broker-tls"))]
    #[test]
    fn https_targets_are_rejected_when_feature_disabled() {
        let err = super::ensure_https_target_supported("builder-a").unwrap_err();
        assert!(
            err.to_string().contains(
                "target `builder-a` uses https://; https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }

// crates/remote-exec-daemon/src/tls.rs
    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn serve_tls_is_rejected_when_feature_disabled() {
        let config = Arc::new(crate::config::DaemonConfig {
            target: "builder-a".to_string(),
            listen: "127.0.0.1:9443".parse().unwrap(),
            default_workdir: std::path::PathBuf::from("."),
            windows_posix_root: None,
            transport: crate::config::DaemonTransport::Tls,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            allow_login_shell: true,
            pty: crate::config::PtyMode::Auto,
            default_shell: None,
            yield_time: crate::config::YieldTimeConfig::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: crate::config::ProcessEnvironment::capture_current(),
            tls: None,
        });

        let err = super::serve_tls(axum::Router::new(), config).await.unwrap_err();
        assert!(
            err.to_string().contains(super::FEATURE_REQUIRED_MESSAGE),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn serve_tls_with_shutdown_is_rejected_when_feature_disabled() {
        let config = Arc::new(crate::config::DaemonConfig {
            target: "builder-a".to_string(),
            listen: "127.0.0.1:9443".parse().unwrap(),
            default_workdir: std::path::PathBuf::from("."),
            windows_posix_root: None,
            transport: crate::config::DaemonTransport::Tls,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            allow_login_shell: true,
            pty: crate::config::PtyMode::Auto,
            default_shell: None,
            yield_time: crate::config::YieldTimeConfig::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: crate::config::ProcessEnvironment::capture_current(),
            tls: None,
        });

        let err = super::serve_tls_with_shutdown(axum::Router::new(), config, async {})
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(super::FEATURE_REQUIRED_MESSAGE),
            "unexpected error: {err}",
        );
    }
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-broker --no-default-features --lib --locked`
Expected: FAIL first because the new broker disabled-path tests are not implemented yet.

Run: `cargo test -p remote-exec-daemon --no-default-features --lib --locked`
Expected: FAIL first because the new daemon disabled-path tests are not implemented yet.

- [ ] **Step 3: Implement the change**

```rust
// crates/remote-exec-broker/src/broker_tls.rs
#[cfg(test)]
mod tests {
    #[test]
    fn broker_http_urls_remain_supported() {
        super::ensure_broker_url_supported("http://127.0.0.1:8787/mcp").unwrap();
    }

    #[cfg(feature = "broker-tls")]
    #[test]
    fn broker_https_urls_are_supported_when_feature_enabled() {
        super::ensure_broker_url_supported("https://broker.example.com/mcp").unwrap();
    }

    #[cfg(not(feature = "broker-tls"))]
    #[test]
    fn broker_https_urls_are_rejected_when_feature_disabled() {
        let err = super::ensure_broker_url_supported("https://broker.example.com/mcp").unwrap_err();
        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "broker-tls"))]
    #[test]
    fn https_targets_are_rejected_when_feature_disabled() {
        let err = super::ensure_https_target_supported("builder-a").unwrap_err();
        assert!(
            err.to_string().contains(
                "target `builder-a` uses https://; https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn build_daemon_https_client_is_rejected_when_feature_disabled() {
        let config: crate::config::TargetConfig = toml::from_str(
            r#"
base_url = "https://127.0.0.1:8443"
ca_pem = "/tmp/ca.pem"
client_cert_pem = "/tmp/client.pem"
client_key_pem = "/tmp/client.key"
"#,
        )
        .unwrap();

        let err = super::build_daemon_https_client(&config).await.unwrap_err();
        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}",
        );
    }
}

// crates/remote-exec-daemon/src/tls.rs
#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::Router;

    use crate::config::{DaemonConfig, DaemonTransport, ProcessEnvironment, PtyMode, YieldTimeConfig};

    #[cfg(not(feature = "tls"))]
    fn tls_transport_config() -> Arc<DaemonConfig> {
        Arc::new(DaemonConfig {
            target: "builder-a".to_string(),
            listen: "127.0.0.1:9443".parse().unwrap(),
            default_workdir: PathBuf::from("."),
            windows_posix_root: None,
            transport: DaemonTransport::Tls,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            allow_login_shell: true,
            pty: PtyMode::Auto,
            default_shell: None,
            yield_time: YieldTimeConfig::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: ProcessEnvironment::capture_current(),
            tls: None,
        })
    }

    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn serve_tls_is_rejected_when_feature_disabled() {
        let err = super::serve_tls(Router::new(), tls_transport_config())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(super::FEATURE_REQUIRED_MESSAGE),
            "unexpected error: {err}",
        );
    }

    #[cfg(not(feature = "tls"))]
    #[tokio::test]
    async fn serve_tls_with_shutdown_is_rejected_when_feature_disabled() {
        let err = super::serve_tls_with_shutdown(Router::new(), tls_transport_config(), async {})
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains(super::FEATURE_REQUIRED_MESSAGE),
            "unexpected error: {err}",
        );
    }
}
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --no-default-features --lib --locked`
Expected: PASS with the broker disabled-path tests included.

Run: `cargo test -p remote-exec-daemon --no-default-features --lib --locked`
Expected: PASS with the daemon disabled-path tests included.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/broker_tls.rs crates/remote-exec-daemon/src/tls.rs
git commit -m "test: cover tls-disabled helper paths"
```

### Task 2: Add no-default-features CI test and clippy lanes

**Files:**
- Modify: `.github/workflows/ci.yml`
- Test/Verify: `cargo test -p remote-exec-broker --no-default-features --tests --locked`
- Test/Verify: `cargo test -p remote-exec-daemon --no-default-features --tests --locked`
- Test/Verify: `cargo clippy -p remote-exec-broker --no-default-features --all-targets --locked -- -D warnings`
- Test/Verify: `cargo clippy -p remote-exec-daemon --no-default-features --all-targets --locked -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: The behavior is CI wiring rather than new runtime logic. The important proof is that the intended no-default-features commands succeed locally before the workflow starts enforcing them remotely.

- [ ] **Step 1: Capture current no-default-features verification surface**

```bash
cargo test -p remote-exec-broker --no-default-features --tests --locked
cargo test -p remote-exec-daemon --no-default-features --tests --locked
cargo clippy -p remote-exec-broker --no-default-features --all-targets --locked -- -D warnings
cargo clippy -p remote-exec-daemon --no-default-features --all-targets --locked -- -D warnings
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `cargo test -p remote-exec-broker --no-default-features --tests --locked`
Expected: PASS, showing the existing broker HTTP-only integration surface is suitable for a dedicated no-default-features job.

Run: `cargo test -p remote-exec-daemon --no-default-features --tests --locked`
Expected: PASS, showing daemon tests already separate TLS-specific coverage behind feature gates.

- [ ] **Step 3: Implement the change**

```yaml
# .github/workflows/ci.yml
  rust-test-no-default-features:
    name: Rust no-default-features (ubuntu-latest)
    runs-on: ubuntu-latest
    timeout-minutes: 45

    steps:
      - name: Checkout
        uses: actions/checkout@v4
        with:
          submodules: recursive

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable

      - name: Cache Rust dependencies
        uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true

      - name: Test broker without default features
        run: cargo test -p remote-exec-broker --no-default-features --tests --locked

      - name: Test daemon without default features
        run: cargo test -p remote-exec-daemon --no-default-features --tests --locked

  rust-clippy-no-default-features:
    name: Rust clippy no-default-features (ubuntu-latest)
    runs-on: ubuntu-latest
    timeout-minutes: 45

    steps:
      - name: Checkout
        uses: actions/checkout@v4
        with:
          submodules: recursive

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy

      - name: Cache Rust dependencies
        uses: Swatinem/rust-cache@v2
        with:
          cache-on-failure: true

      - name: Run broker clippy without default features
        run: cargo clippy -p remote-exec-broker --no-default-features --all-targets --locked -- -D warnings

      - name: Run daemon clippy without default features
        run: cargo clippy -p remote-exec-daemon --no-default-features --all-targets --locked -- -D warnings
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo test -p remote-exec-broker --no-default-features --tests --locked`
Expected: PASS.

Run: `cargo test -p remote-exec-daemon --no-default-features --tests --locked`
Expected: PASS.

Run: `cargo clippy -p remote-exec-broker --no-default-features --all-targets --locked -- -D warnings`
Expected: PASS.

Run: `cargo clippy -p remote-exec-daemon --no-default-features --all-targets --locked -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: exercise no-default-features builds"
```

### Task 3: Document the explicit no-default-features verification path

**Files:**
- Modify: `README.md`
- Test/Verify: `cargo fmt --all --check`

**Testing approach:** `no new tests needed`
Reason: This task only updates the documented quality gate and contributor verification commands to match the new CI coverage.

- [ ] **Step 1: Update the quality gate and focused verification docs**

```markdown
# README.md
- Under the focused testing section, add:
  - `cargo test -p remote-exec-broker --no-default-features --tests`
  - `cargo test -p remote-exec-daemon --no-default-features --tests`
  - `cargo clippy -p remote-exec-broker --no-default-features --all-targets -- -D warnings`
  - `cargo clippy -p remote-exec-daemon --no-default-features --all-targets -- -D warnings`

- Under the quality gate / CI coverage section, add a short note such as:
  - "CI also exercises broker and daemon no-default-features builds on Ubuntu to keep the tls-disabled code paths intentionally tested."
```

- [ ] **Step 2: Run the focused verification for this step**

Run: `rg -n "no-default-features|tls-disabled" README.md`
Expected: The new no-default-features guidance appears in the focused testing and quality gate sections.

- [ ] **Step 3: Implement the change**

```markdown
# README.md
Relevant focused commands:
- `cargo test -p remote-exec-broker --test mcp_exec`
- `cargo test -p remote-exec-broker --test mcp_assets`
- `cargo test -p remote-exec-broker --test mcp_transfer`
- `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- `cargo test -p remote-exec-broker --no-default-features --tests`
- `cargo test -p remote-exec-daemon --test exec_rpc`
- `cargo test -p remote-exec-daemon --test patch_rpc`
- `cargo test -p remote-exec-daemon --test image_rpc`
- `cargo test -p remote-exec-daemon --test transfer_rpc`
- `cargo test -p remote-exec-daemon --test health`
- `cargo test -p remote-exec-daemon --no-default-features --tests`
- `cargo clippy -p remote-exec-broker --no-default-features --all-targets -- -D warnings`
- `cargo clippy -p remote-exec-daemon --no-default-features --all-targets -- -D warnings`

Quality gate:
- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

Additional CI coverage:
- Ubuntu CI also runs broker and daemon `--no-default-features` test and clippy jobs so the `tls-disabled` code paths stay intentionally exercised.
```

- [ ] **Step 4: Run the post-change verification**

Run: `cargo fmt --all --check`
Expected: PASS with no formatting changes required by the documentation update.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/superpowers/plans/2026-05-05-no-default-features-hardening.md
git commit -m "docs: document no-default-features coverage"
```
