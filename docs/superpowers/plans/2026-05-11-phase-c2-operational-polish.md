# Phase C2 Operational Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve the Phase C2 operational items deferred from `docs/CODE_AUDIT_ROUND2.md`: bounded broker-daemon RPCs, parallel bounded startup probes, and configurable C++ HTTP idle timeout.

**Architecture:** Keep C2 operational rather than structural. Broker remote targets get a small per-target timeout policy used by both client construction and startup probing, JSON daemon RPCs become explicitly bounded without applying a whole-client timeout to transfer streams, and startup builds all remote target handles concurrently while preserving deterministic target insertion order. The C++ daemon exposes the existing idle timeout as ordinary config and keeps its default behavior unchanged.

**Tech Stack:** Rust 2024 with Tokio, reqwest, Serde/TOML, futures-util, rmcp integration tests; C++11 daemon with GNU/BSD/NMAKE source inventories and POSIX/Windows XP-compatible build paths.

---

## Scope

Included deferred Phase C2 audit items:

- `#27` broker-to-daemon per-RPC timeout and reqwest connect/read timeout policy.
- `#28` parallel, bounded broker startup target probes.
- `#37` C++ `HTTP_CONNECTION_IDLE_TIMEOUT_MS` config surface.

Out of scope:

- Phase C1 cleanup/refactor items already implemented in `docs/superpowers/plans/2026-05-11-phase-c1-cleanups-refactors.md`.
- Port tunnel upgrade and preface timeout redesign; those already have explicit constants in `daemon_client.rs`.
- A whole-client reqwest `.timeout(...)` for daemon clients. That would also cap long transfer streams, so C2 should use `connect_timeout`, `read_timeout`, and a JSON-RPC-specific `tokio::time::timeout` wrapper around `DaemonClient::post`.

## Current Validation Snapshot

- `crates/remote-exec-broker/src/config.rs` has no remote-target timeout config.
- `crates/remote-exec-broker/src/daemon_client.rs` only bounds port tunnel upgrade/preface; `post()` has no explicit timeout.
- `crates/remote-exec-broker/src/startup.rs` still loops over configured remote targets one by one.
- `crates/remote-exec-daemon-cpp/src/http_connection.cpp` hard-codes `HTTP_CONNECTION_IDLE_TIMEOUT_MS = 30000UL`.

## File Structure

- `crates/remote-exec-broker/src/config.rs`: add per-target timeout config, validation, duration helpers, and config tests.
- `crates/remote-exec-broker/src/daemon_client.rs`: apply reqwest connect/read defaults and bound JSON daemon RPCs.
- `crates/remote-exec-broker/src/broker_tls_enabled.rs`: apply the same reqwest connect/read defaults to HTTPS daemon clients.
- `crates/remote-exec-broker/src/startup.rs`: probe configured remote targets concurrently and apply startup-probe timeout.
- `configs/broker.example.toml` and `README.md`: document broker target timeout knobs and startup semantics.
- `crates/remote-exec-daemon-cpp/include/config.h`, `src/config.cpp`, `src/http_connection.cpp`, `tests/test_config.cpp`: expose and validate C++ HTTP idle timeout.
- `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini` and `crates/remote-exec-daemon-cpp/README.md`: document the C++ timeout knob.

---

### Task 1: Save The Phase C2 Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-c2-operational-polish.md`
- Test/Verify: `test -f docs/superpowers/plans/2026-05-11-phase-c2-operational-polish.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked implementation plan artifact only.

- [ ] **Step 1: Verify the plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-c2-operational-polish.md`
Expected: command exits successfully.

- [ ] **Step 2: Verify the plan is limited to C2.**

Run: `sed -n '1,80p' docs/superpowers/plans/2026-05-11-phase-c2-operational-polish.md`
Expected: output names `#27`, `#28`, and `#37`, and does not add C1 refactor work.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-c2-operational-polish.md
git commit -m "docs: plan phase c2 operational polish"
```

### Task 2: Add Broker Remote Target Timeout Config

**Finding:** `#27` and `#28`

**Files:**
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `configs/broker.example.toml`
- Modify: `README.md`
- Test/Verify:
  - `cargo test -p remote-exec-broker config::tests::load_accepts_remote_target_timeout_config`
  - `cargo test -p remote-exec-broker config::tests::load_rejects_zero_remote_target_timeout`

**Testing approach:** TDD
Reason: This changes TOML shape and validation. The behavior has a clear config parse/validation seam.

- [ ] **Step 1: Write failing config tests.**

Add these tests inside `#[cfg(test)] mod tests` in `crates/remote-exec-broker/src/config.rs`:

```rust
    #[tokio::test]
    async fn load_accepts_remote_target_timeout_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-a]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-a.timeouts]
connect_ms = 1234
read_ms = 2345
request_ms = 3456
startup_probe_ms = 4567
"#,
        )
        .await
        .unwrap();

        let config = BrokerConfig::load(&config_path).await.unwrap();
        let timeouts = config.targets["builder-a"].timeouts;
        assert_eq!(timeouts.connect_ms, 1234);
        assert_eq!(timeouts.read_ms, 2345);
        assert_eq!(timeouts.request_ms, 3456);
        assert_eq!(timeouts.startup_probe_ms, 4567);
        assert_eq!(
            timeouts.request_timeout(),
            std::time::Duration::from_millis(3456)
        );
    }

    #[tokio::test]
    async fn load_rejects_zero_remote_target_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("broker.toml");
        tokio::fs::write(
            &config_path,
            r#"[targets.builder-a]
base_url = "http://127.0.0.1:8181"
allow_insecure_http = true

[targets.builder-a.timeouts]
request_ms = 0
"#,
        )
        .await
        .unwrap();

        let err = BrokerConfig::load(&config_path).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("target `builder-a` timeouts.request_ms must be greater than zero"),
            "unexpected error: {err}"
        );
    }
```

- [ ] **Step 2: Run the failing tests.**

Run:

```bash
cargo test -p remote-exec-broker config::tests::load_accepts_remote_target_timeout_config
cargo test -p remote-exec-broker config::tests::load_rejects_zero_remote_target_timeout
```

Expected: the tests fail to compile because `TargetConfig::timeouts` and `TargetTimeoutConfig` do not exist yet.

- [ ] **Step 3: Add the timeout config type and validation.**

In `crates/remote-exec-broker/src/config.rs`, add `Duration` to the imports:

```rust
use std::time::Duration;
```

Add the field to `TargetConfig` after `http_auth`:

```rust
    #[serde(default)]
    pub timeouts: TargetTimeoutConfig,
```

Add this type near `TargetConfig`:

```rust
const DEFAULT_TARGET_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_TARGET_READ_TIMEOUT_MS: u64 = 310_000;
const DEFAULT_TARGET_REQUEST_TIMEOUT_MS: u64 = 310_000;
const DEFAULT_TARGET_STARTUP_PROBE_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub struct TargetTimeoutConfig {
    #[serde(default = "default_target_connect_timeout_ms")]
    pub connect_ms: u64,
    #[serde(default = "default_target_read_timeout_ms")]
    pub read_ms: u64,
    #[serde(default = "default_target_request_timeout_ms")]
    pub request_ms: u64,
    #[serde(default = "default_target_startup_probe_timeout_ms")]
    pub startup_probe_ms: u64,
}

impl Default for TargetTimeoutConfig {
    fn default() -> Self {
        Self {
            connect_ms: DEFAULT_TARGET_CONNECT_TIMEOUT_MS,
            read_ms: DEFAULT_TARGET_READ_TIMEOUT_MS,
            request_ms: DEFAULT_TARGET_REQUEST_TIMEOUT_MS,
            startup_probe_ms: DEFAULT_TARGET_STARTUP_PROBE_TIMEOUT_MS,
        }
    }
}

impl TargetTimeoutConfig {
    pub(crate) fn validate(&self, target_name: &str) -> anyhow::Result<()> {
        validate_timeout_ms(target_name, "connect_ms", self.connect_ms)?;
        validate_timeout_ms(target_name, "read_ms", self.read_ms)?;
        validate_timeout_ms(target_name, "request_ms", self.request_ms)?;
        validate_timeout_ms(target_name, "startup_probe_ms", self.startup_probe_ms)?;
        Ok(())
    }

    pub(crate) fn connect_timeout(self) -> Duration {
        Duration::from_millis(self.connect_ms)
    }

    pub(crate) fn read_timeout(self) -> Duration {
        Duration::from_millis(self.read_ms)
    }

    pub(crate) fn request_timeout(self) -> Duration {
        Duration::from_millis(self.request_ms)
    }

    pub(crate) fn startup_probe_timeout(self) -> Duration {
        Duration::from_millis(self.startup_probe_ms)
    }
}

fn default_target_connect_timeout_ms() -> u64 {
    DEFAULT_TARGET_CONNECT_TIMEOUT_MS
}

fn default_target_read_timeout_ms() -> u64 {
    DEFAULT_TARGET_READ_TIMEOUT_MS
}

fn default_target_request_timeout_ms() -> u64 {
    DEFAULT_TARGET_REQUEST_TIMEOUT_MS
}

fn default_target_startup_probe_timeout_ms() -> u64 {
    DEFAULT_TARGET_STARTUP_PROBE_TIMEOUT_MS
}

fn validate_timeout_ms(target_name: &str, field: &str, value: u64) -> anyhow::Result<()> {
    anyhow::ensure!(
        value > 0,
        "target `{target_name}` timeouts.{field} must be greater than zero"
    );
    Ok(())
}
```

Update `TargetConfig::validated_transport` so timeout validation runs before transport-specific checks:

```rust
    pub(crate) fn validated_transport(&self, name: &str) -> anyhow::Result<TargetTransportKind> {
        self.timeouts.validate(name)?;

        if let Some(http_auth) = &self.http_auth {
            http_auth.validate(&format!("target `{name}`"))?;
        }
```

- [ ] **Step 4: Document broker timeout config.**

In `configs/broker.example.toml`, add this block under one HTTPS target and before `[targets.builder-b]`:

```toml
# Optional remote-daemon timeout policy. Defaults are shown.
#[targets.builder-a.timeouts]
#connect_ms = 5000
#read_ms = 310000
#request_ms = 310000
#startup_probe_ms = 5000
```

In `README.md`, add these bullets to the "Broker config covers one entry per target" list after "optional HTTP bearer auth shared secret for daemon requests":

```markdown
- optional per-target daemon timeout policy under `[targets.<name>.timeouts]`
- separate startup probe timeout so slow or wedged daemons do not serialize broker startup
```

In the "Reliability Notes" section, replace the two startup bullets with:

```markdown
- The broker starts even if some configured targets are temporarily unreachable.
- Remote target startup probes run concurrently and are bounded by each target's `timeouts.startup_probe_ms`; targets that are unavailable at broker startup are verified before the first forwarded call.
```

- [ ] **Step 5: Run focused config verification.**

Run:

```bash
cargo test -p remote-exec-broker config::tests::load_accepts_remote_target_timeout_config
cargo test -p remote-exec-broker config::tests::load_rejects_zero_remote_target_timeout
```

Expected: both tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/config.rs configs/broker.example.toml README.md
git commit -m "feat: add broker daemon timeout config"
```

### Task 3: Bound Broker JSON Daemon RPCs And Apply Reqwest Defaults

**Finding:** `#27`

**Files:**
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/broker_tls_enabled.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker daemon_client::tests::daemon_rpc_times_out_hung_response`
  - `cargo test -p remote-exec-broker daemon_client::tests::daemon_request_still_applies_authorization_header`
  - `cargo test -p remote-exec-broker daemon_client::tests::port_tunnel_sends_upgrade_headers_and_preface`

**Testing approach:** TDD
Reason: The behavior is visible at the daemon client seam: a server that accepts a request and never returns a response must produce a bounded transport error.

- [ ] **Step 1: Write the failing daemon client timeout test.**

In `crates/remote-exec-broker/src/daemon_client.rs`, update the test imports:

```rust
    use std::time::Duration;

    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
```

Add this test inside the existing `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn daemon_rpc_times_out_hung_response() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = DaemonClient {
            client: reqwest::Client::builder().build().unwrap(),
            target_name: "builder-a".to_string(),
            base_url: format!("http://{addr}"),
            authorization: None,
            request_timeout: Duration::from_millis(50),
        };

        let started = std::time::Instant::now();
        let err = client.target_info().await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout took too long: {:?}",
            started.elapsed()
        );
        assert!(
            err.to_string()
                .contains("daemon rpc `/v1/target-info` timed out after 50 ms"),
            "unexpected error: {err}"
        );
        server.abort();
    }
```

- [ ] **Step 2: Run the failing daemon client timeout test.**

Run: `cargo test -p remote-exec-broker daemon_client::tests::daemon_rpc_times_out_hung_response`
Expected: compile fails because `DaemonClient::request_timeout` does not exist.

- [ ] **Step 3: Add timeout state and reqwest client helper.**

In `crates/remote-exec-broker/src/daemon_client.rs`, update the config import:

```rust
use crate::config::{TargetConfig, TargetTimeoutConfig, TargetTransportKind};
```

Add a field to `DaemonClient`:

```rust
    request_timeout: std::time::Duration,
```

Add these helper functions near `build_bearer_authorization_header`:

```rust
pub(crate) fn apply_daemon_client_timeouts(
    builder: reqwest::ClientBuilder,
    timeouts: TargetTimeoutConfig,
) -> reqwest::ClientBuilder {
    builder
        .connect_timeout(timeouts.connect_timeout())
        .read_timeout(timeouts.read_timeout())
}

fn build_http_daemon_client(timeouts: TargetTimeoutConfig) -> anyhow::Result<reqwest::Client> {
    apply_daemon_client_timeouts(reqwest::Client::builder(), timeouts)
        .build()
        .map_err(anyhow::Error::from)
}
```

Update `DaemonClient::new`:

```rust
        let timeouts = config.timeouts;
        let client = match config.validated_transport(&target_name)? {
            TargetTransportKind::Http => build_http_daemon_client(timeouts)?,
            TargetTransportKind::Https => {
                crate::broker_tls::build_daemon_https_client(config).await?
            }
        };
```

Return the timeout in the constructed client:

```rust
            request_timeout: timeouts.request_timeout(),
```

Update the existing test helper `test_client` so all test client literals include:

```rust
            request_timeout: crate::config::TargetTimeoutConfig::default().request_timeout(),
```

Update the `port_tunnel_sends_upgrade_headers_and_preface` test client literal the same way.

- [ ] **Step 4: Wrap `DaemonClient::post` in an explicit timeout.**

Replace the body of `DaemonClient::post` with this structure, preserving the existing logging fields:

```rust
        let started = std::time::Instant::now();
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            "sending daemon rpc"
        );

        let result = tokio::time::timeout(self.request_timeout, async {
            let response = self
                .request(path)
                .json(body)
                .send()
                .await
                .map_err(|err| self.rpc_transport_error(path, started, err))?;
            let response = self.ensure_rpc_success(path, started, response).await?;

            let bytes = response.bytes().await.map_err(|err| {
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "daemon rpc body read failed"
                );
                DaemonClientError::Decode(err.into())
            })?;
            let decoded = serde_json::from_slice(&bytes).map_err(|err| {
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "daemon rpc decode failed"
                );
                DaemonClientError::Decode(err.into())
            })?;
            tracing::debug!(
                target = %self.target_name,
                base_url = %self.base_url,
                path,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "daemon rpc completed"
            );
            Ok(decoded)
        })
        .await;

        match result {
            Ok(result) => result,
            Err(_) => {
                let timeout_ms = self.request_timeout.as_millis() as u64;
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    timeout_ms,
                    "daemon rpc timed out"
                );
                Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "daemon rpc `{path}` timed out after {timeout_ms} ms"
                )))
            }
        }
```

- [ ] **Step 5: Apply reqwest defaults to HTTPS daemon clients.**

In `crates/remote-exec-broker/src/broker_tls_enabled.rs`, replace both `reqwest::Client::builder()` calls that construct daemon clients with:

```rust
crate::daemon_client::apply_daemon_client_timeouts(
    reqwest::Client::builder(),
    config.timeouts,
)
```

Keep the existing TLS configuration chained after that builder.

- [ ] **Step 6: Run focused daemon client verification.**

Run:

```bash
cargo test -p remote-exec-broker daemon_client::tests::daemon_rpc_times_out_hung_response
cargo test -p remote-exec-broker daemon_client::tests::daemon_request_still_applies_authorization_header
cargo test -p remote-exec-broker daemon_client::tests::port_tunnel_sends_upgrade_headers_and_preface
```

Expected: all tests pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-broker/src/daemon_client.rs crates/remote-exec-broker/src/broker_tls_enabled.rs
git commit -m "fix: bound broker daemon rpc calls"
```

### Task 4: Probe Remote Targets Concurrently During Startup

**Finding:** `#28`

**Files:**
- Modify: `crates/remote-exec-broker/src/startup.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker startup::tests::remote_startup_probes_are_parallel_and_bounded`
  - `cargo test -p remote-exec-broker --test mcp_assets list_targets_returns_cached_daemon_info_and_null_for_unavailable_targets`

**Testing approach:** TDD
Reason: Startup ordering and timeout behavior can be tested at `build_state` without launching the full MCP server.

- [ ] **Step 1: Write the failing startup concurrency test.**

In `crates/remote-exec-broker/src/startup.rs`, update the test imports:

```rust
    use std::collections::BTreeMap;
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use crate::config::{BrokerConfig, LocalTargetConfig, TargetConfig, TargetTimeoutConfig};
```

Add these helpers to the test module:

```rust
    async fn spawn_hung_target_info_server(delay: Duration) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(value) => value,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    tokio::time::sleep(delay).await;
                });
            }
        });
        addr
    }

    fn remote_http_target(addr: std::net::SocketAddr, startup_probe_ms: u64) -> TargetConfig {
        TargetConfig {
            base_url: format!("http://{addr}"),
            http_auth: None,
            timeouts: TargetTimeoutConfig {
                startup_probe_ms,
                request_ms: 5_000,
                ..TargetTimeoutConfig::default()
            },
            ca_pem: None,
            client_cert_pem: None,
            client_key_pem: None,
            allow_insecure_http: true,
            skip_server_name_verification: false,
            pinned_server_cert_pem: None,
            expected_daemon_name: None,
        }
    }
```

Add this test:

```rust
    #[tokio::test]
    async fn remote_startup_probes_are_parallel_and_bounded() {
        let mut targets = BTreeMap::new();
        for index in 0..4 {
            let addr = spawn_hung_target_info_server(Duration::from_secs(5)).await;
            targets.insert(
                format!("slow-{index}"),
                remote_http_target(addr, 400),
            );
        }

        let started = std::time::Instant::now();
        let state = build_state(BrokerConfig {
            mcp: Default::default(),
            host_sandbox: None,
            enable_transfer_compression: true,
            transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
            disable_structured_content: false,
            port_forward_limits: Default::default(),
            targets,
            local: None,
        })
        .await
        .unwrap();

        assert!(
            started.elapsed() < Duration::from_millis(1_200),
            "startup probes did not run concurrently: {:?}",
            started.elapsed()
        );
        assert_eq!(state.targets.len(), 4);
        for handle in state.targets.values() {
            assert_eq!(handle.cached_daemon_info().await, None);
        }
    }
```

- [ ] **Step 2: Run the failing startup concurrency test.**

Run: `cargo test -p remote-exec-broker startup::tests::remote_startup_probes_are_parallel_and_bounded`
Expected: the test fails by taking roughly `4 * startup_probe_ms` because startup probes still run serially.

- [ ] **Step 3: Make remote startup probes concurrent.**

In `crates/remote-exec-broker/src/startup.rs`, replace `insert_remote_targets` with:

```rust
async fn insert_remote_targets(
    target_configs: &BTreeMap<String, config::TargetConfig>,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    let probes = target_configs.iter().map(|(name, target_config)| async move {
        (
            name.clone(),
            build_remote_target_handle(name, target_config).await,
        )
    });

    for (name, handle) in futures_util::future::join_all(probes).await {
        targets.insert(name, handle?);
    }
    Ok(())
}
```

- [ ] **Step 4: Bound startup target-info probes separately from regular RPC timeout.**

In `build_remote_target_handle`, replace:

```rust
    match client.target_info().await {
```

with:

```rust
    match tokio::time::timeout(
        target_config.timeouts.startup_probe_timeout(),
        client.target_info(),
    )
    .await
    {
        Err(_) => {
            log_remote_target_startup_probe_timeout(name, target_config);
            Ok(TargetHandle::unavailable(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Ok(Ok(info)) => {
```

Change the original `Ok(info)` arm body only by adding the extra `Ok(` layer shown above, and change the original error arms to `Ok(Err(...))` patterns:

```rust
        Ok(Err(DaemonClientError::Transport(err))) => {
            log_remote_target_unavailable(name, target_config, &err);
            Ok(TargetHandle::unavailable(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Ok(Err(err)) => Err(err.into()),
```

Add this logging helper near `log_remote_target_unavailable`:

```rust
fn log_remote_target_startup_probe_timeout(
    name: &str,
    target_config: &config::TargetConfig,
) {
    tracing::warn!(
        target = %name,
        http_auth_enabled = target_config.http_auth.is_some(),
        timeout_ms = target_config.timeouts.startup_probe_ms,
        "target unavailable during broker startup: startup probe timed out"
    );
}
```

- [ ] **Step 5: Run focused startup verification.**

Run:

```bash
cargo test -p remote-exec-broker startup::tests::remote_startup_probes_are_parallel_and_bounded
cargo test -p remote-exec-broker --test mcp_assets list_targets_returns_cached_daemon_info_and_null_for_unavailable_targets
```

Expected: both tests pass. The startup concurrency test should complete in less than `1.2s`.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/startup.rs
git commit -m "fix: parallelize bounded broker startup probes"
```

### Task 5: Expose C++ HTTP Idle Timeout In Config

**Finding:** `#37`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-config`
  - `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`

**Testing approach:** TDD
Reason: Config parsing and validation can be driven from the existing C++ config test. Building/running server streaming verifies the HTTP connection code still links and operates.

- [ ] **Step 1: Write failing C++ config assertions.**

In `crates/remote-exec-daemon-cpp/tests/test_config.cpp`, add this config line after `max_request_body_bytes = 1048576\n`:

```cpp
        "http_connection_idle_timeout_ms = 9000\n"
```

Add this assertion after `assert(config.max_request_body_bytes == 1048576UL);`:

```cpp
    assert(config.http_connection_idle_timeout_ms == 9000UL);
```

Add this default assertion in the sandbox config block after `assert(sandbox_config.port_forward_limits.connect_timeout_ms == DEFAULT_PORT_FORWARD_CONNECT_TIMEOUT_MS);`:

```cpp
    assert(sandbox_config.http_connection_idle_timeout_ms == DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS);
```

Add `"http_connection_idle_timeout_ms",` to the `invalid_limit_keys` array.

- [ ] **Step 2: Run the failing C++ config test.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-config`
Expected: compile fails because `DaemonConfig::http_connection_idle_timeout_ms` and `DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS` do not exist.

- [ ] **Step 3: Add the config field and default.**

In `crates/remote-exec-daemon-cpp/include/config.h`, add this field to `DaemonConfig` after `max_request_body_bytes`:

```cpp
    unsigned long http_connection_idle_timeout_ms;
```

Add this constant near the other defaults:

```cpp
static const unsigned long DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS = 30000UL;
```

In `crates/remote-exec-daemon-cpp/src/config.cpp`, set the field in `load_config` after `max_request_body_bytes`:

```cpp
    config.http_connection_idle_timeout_ms = read_optional_unsigned_long(
        values,
        "http_connection_idle_timeout_ms",
        DEFAULT_HTTP_CONNECTION_IDLE_TIMEOUT_MS
    );
```

In `validate_daemon_config`, add:

```cpp
    if (config.http_connection_idle_timeout_ms == 0) {
        throw std::runtime_error("http_connection_idle_timeout_ms must be greater than zero");
    }
```

- [ ] **Step 4: Use the configured timeout in the HTTP connection loop.**

In `crates/remote-exec-daemon-cpp/src/http_connection.cpp`, remove:

```cpp
const unsigned long HTTP_CONNECTION_IDLE_TIMEOUT_MS = 30000UL;
```

Replace:

```cpp
            set_socket_timeout_ms(client.get(), HTTP_CONNECTION_IDLE_TIMEOUT_MS);
```

with:

```cpp
            set_socket_timeout_ms(
                client.get(),
                state.config.http_connection_idle_timeout_ms
            );
```

- [ ] **Step 5: Document the C++ config knob.**

In `crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini`, add this line under `# max_request_body_bytes = 536870912`:

```ini
# http_connection_idle_timeout_ms = 30000
```

In `crates/remote-exec-daemon-cpp/README.md`, add the same line in the example config under `# max_request_body_bytes = 536870912`, and add this sentence after the paragraph that starts "Logs go to `stderr`":

```markdown
Idle keep-alive HTTP connections wait up to `http_connection_idle_timeout_ms`
for the next request header before the daemon closes the socket.
```

- [ ] **Step 6: Run focused C++ verification.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-config
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: both tests pass.

- [ ] **Step 7: Commit.**

```bash
git add \
  crates/remote-exec-daemon-cpp/include/config.h \
  crates/remote-exec-daemon-cpp/src/config.cpp \
  crates/remote-exec-daemon-cpp/src/http_connection.cpp \
  crates/remote-exec-daemon-cpp/tests/test_config.cpp \
  crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini \
  crates/remote-exec-daemon-cpp/README.md
git commit -m "feat: expose cpp http idle timeout config"
```

### Task 6: Phase C2 Integration Verification

**Files:**
- Verify: Rust broker config, daemon client, startup, integration tests.
- Verify: C++ POSIX daemon checks.
- Verify: formatting and clippy.

**Testing approach:** existing tests + targeted verification
Reason: C2 crosses broker config, daemon HTTP client construction, startup behavior, docs, and C++ daemon config/runtime code.

- [ ] **Step 1: Run broker-focused tests.**

Run:

```bash
cargo test -p remote-exec-broker config::tests::load_accepts_remote_target_timeout_config
cargo test -p remote-exec-broker config::tests::load_rejects_zero_remote_target_timeout
cargo test -p remote-exec-broker daemon_client::tests::daemon_rpc_times_out_hung_response
cargo test -p remote-exec-broker startup::tests::remote_startup_probes_are_parallel_and_bounded
cargo test -p remote-exec-broker --test mcp_assets
```

Expected: all commands pass.

- [ ] **Step 2: Run C++ focused tests.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-config
make -C crates/remote-exec-daemon-cpp test-host-server-streaming
```

Expected: both commands pass.

- [ ] **Step 3: Run formatting.**

Run: `cargo fmt --all --check`
Expected: command exits successfully.

- [ ] **Step 4: Run broader project verification.**

Run:

```bash
cargo test -p remote-exec-broker --test multi_target -- --nocapture
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
make -C crates/remote-exec-daemon-cpp check-posix
```

Expected: all commands pass. If `cargo test --workspace` exposes unrelated environmental flakiness, rerun the exact failing test once and record both outputs in the final handoff.

- [ ] **Step 5: Commit any integration fallout.**

Only run this commit if Step 1 through Step 4 required code or docs fixes after Task 5:

```bash
git add crates/remote-exec-broker crates/remote-exec-daemon-cpp configs README.md
git commit -m "fix: resolve phase c2 integration fallout"
```

---

## Self-Review

- Spec coverage: `#27` is covered by Tasks 2 and 3, `#28` by Tasks 2 and 4, and `#37` by Task 5. Task 6 verifies cross-cutting integration.
- Placeholder scan: the plan contains no deferred implementation blanks.
- Type consistency: the timeout config is consistently named `TargetTimeoutConfig`; TOML fields are consistently `connect_ms`, `read_ms`, `request_ms`, and `startup_probe_ms`; C++ config is consistently `http_connection_idle_timeout_ms`.

## Execution Handoff

The user previously selected Plan-Based Execution for this tree. Continue with `superpowers:executing-plans` task-by-task and commit after each task unless the user explicitly switches execution style.
