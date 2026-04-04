# Apply Patch Exec Interception Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Intercept explicit `apply_patch` and `applypatch` shell-style `exec_command` requests in the broker, route them through the existing patch RPC path, and return Codex-style wrapped unified-exec output.

**Architecture:** Keep interception broker-local. Add a narrow parser module for documented command forms, reuse a shared broker patch-forwarding helper instead of calling daemon `exec_start`, and format intercepted success with a dedicated wrapped exec formatter while leaving direct `apply_patch` behavior unchanged.

**Tech Stack:** Rust 2024, Tokio, rmcp, Axum test stubs, serde/schemars, cargo test

---

## File Map

- Create: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
  Responsibility: recognize explicit intercepted `apply_patch` command text and extract `{ patch, workdir }`.
- Modify: `crates/remote-exec-broker/src/tools/mod.rs`
  Responsibility: register the new interception module in broker tool wiring.
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
  Responsibility: run interception before daemon `exec_start`, skip session allocation on matches, and build wrapped unified-exec-shaped output.
- Modify: `crates/remote-exec-broker/src/tools/patch.rs`
  Responsibility: expose a shared broker-local patch-forwarding helper used by both direct `apply_patch` and intercepted `exec_command`.
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
  Responsibility: add a dedicated formatter for intercepted patch success text.
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs`
  Responsibility: broker integration coverage for direct interception, alias interception, heredoc/cd interception, non-match fallback, and invalid intercepted failure behavior.
- Modify: `crates/remote-exec-broker/tests/support/mod.rs`
  Responsibility: capture stub daemon telemetry for `/v1/exec/start` and `/v1/patch/apply` so interception tests can assert the actual forwarding path.

### Task 1: Add A Narrow Direct-Invocation Interception Parser

**Files:**
- Create: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- Modify: `crates/remote-exec-broker/src/tools/mod.rs`
- Test/Verify: `cargo test -p remote-exec-broker parses_direct_apply_patch_command -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: the interception parser is a pure broker-local seam and can be proven with tight unit tests before it is wired into `exec_command`.

- [ ] **Step 1: Create the interception module with failing unit tests for direct forms**

```rust
// crates/remote-exec-broker/src/tools/exec_intercept.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptedApplyPatch {
    pub patch: String,
    pub workdir: Option<String>,
}

pub fn maybe_intercept_apply_patch(
    _cmd: &str,
    _workdir: Option<&str>,
) -> Option<InterceptedApplyPatch> {
    None
}

#[cfg(test)]
mod tests {
    use super::{InterceptedApplyPatch, maybe_intercept_apply_patch};

    #[test]
    fn parses_direct_apply_patch_command() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: hello.txt\n",
            "+hello\n",
            "*** End Patch\n",
        );
        let cmd = format!("apply_patch '{patch}'");

        assert_eq!(
            maybe_intercept_apply_patch(&cmd, Some("workspace")),
            Some(InterceptedApplyPatch {
                patch: patch.to_string(),
                workdir: Some("workspace".to_string()),
            })
        );
    }

    #[test]
    fn parses_direct_applypatch_alias() {
        let patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: alias.txt\n",
            "+alias\n",
            "*** End Patch\n",
        );
        let cmd = format!("applypatch \"{patch}\"");

        assert_eq!(
            maybe_intercept_apply_patch(&cmd, None),
            Some(InterceptedApplyPatch {
                patch: patch.to_string(),
                workdir: None,
            })
        );
    }

    #[test]
    fn rejects_raw_patch_body_and_extra_commands() {
        let raw_patch = concat!(
            "*** Begin Patch\n",
            "*** Add File: no.txt\n",
            "+no\n",
            "*** End Patch\n",
        );

        assert_eq!(maybe_intercept_apply_patch(raw_patch, None), None);
        assert_eq!(
            maybe_intercept_apply_patch(
                &format!("apply_patch '{raw_patch}' && echo done"),
                None
            ),
            None
        );
    }
}

// crates/remote-exec-broker/src/tools/mod.rs
pub mod exec;
pub mod exec_intercept;
pub mod image;
pub mod patch;
```

- [ ] **Step 2: Run the focused parser verification and confirm it fails first**

Run: `cargo test -p remote-exec-broker parses_direct_apply_patch_command -- --exact --nocapture`
Expected: FAIL because `maybe_intercept_apply_patch(...)` still returns `None`.

- [ ] **Step 3: Implement direct explicit-form parsing and conservative non-matches**

```rust
// crates/remote-exec-broker/src/tools/exec_intercept.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptedApplyPatch {
    pub patch: String,
    pub workdir: Option<String>,
}

pub fn maybe_intercept_apply_patch(
    cmd: &str,
    workdir: Option<&str>,
) -> Option<InterceptedApplyPatch> {
    let patch = parse_direct_invocation(cmd.trim())?;
    Some(InterceptedApplyPatch {
        patch,
        workdir: workdir.map(ToString::to_string),
    })
}

fn parse_direct_invocation(cmd: &str) -> Option<String> {
    ["apply_patch", "applypatch"]
        .into_iter()
        .find_map(|name| {
            let rest = cmd.strip_prefix(name)?.trim_start();
            parse_single_quoted_argument(rest)
        })
}

fn parse_single_quoted_argument(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let end = rest[1..].find(quote)? + 1;
    let patch = &rest[1..end];
    let trailing = &rest[end + 1..];
    if !trailing.trim().is_empty() {
        return None;
    }

    Some(patch.to_string())
}
```

- [ ] **Step 4: Run the focused parser verification again**

Run: `cargo test -p remote-exec-broker parses_direct_apply_patch_command -- --exact --nocapture`
Expected: PASS, proving explicit direct `apply_patch` commands are recognized before any broker wiring is added.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/exec_intercept.rs \
        crates/remote-exec-broker/src/tools/mod.rs
git commit -m "test: add direct apply_patch exec interception parser"
```

### Task 2: Wire Direct Interception Through Broker Exec Output

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/exec.rs:1-133`
- Modify: `crates/remote-exec-broker/src/tools/patch.rs:1-24`
- Modify: `crates/remote-exec-broker/src/mcp_server.rs:144-263`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs:1-226`
- Modify: `crates/remote-exec-broker/tests/support/mod.rs:1-522`
- Test/Verify: `cargo test -p remote-exec-broker exec_command_intercepts_direct_apply_patch_and_wraps_exec_output -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: the next seam is externally observable broker behavior, so the change should be driven from failing end-to-end broker tests plus one formatter unit test.

- [ ] **Step 1: Extend the stub daemon and add failing direct-interception broker tests**

```rust
// crates/remote-exec-broker/tests/support/mod.rs
pub struct BrokerFixture {
    pub _tempdir: TempDir,
    pub client: RunningService<RoleClient, DummyClientHandler>,
    stub_state: Option<StubDaemonState>,
}

impl BrokerFixture {
    pub async fn exec_start_calls(&self) -> usize {
        *self
            .stub_state
            .as_ref()
            .expect("stub daemon state")
            .exec_start_calls
            .lock()
            .await
    }

    pub async fn last_patch_request(&self) -> Option<PatchApplyRequest> {
        self.stub_state
            .as_ref()
            .expect("stub daemon state")
            .last_patch_request
            .lock()
            .await
            .clone()
    }
}

impl DelayedTargetFixture {
    pub async fn spawn_target(&self, target: &str) {
        let state = StubDaemonState {
            target: target.to_string(),
            daemon_instance_id: "daemon-instance-1".to_string(),
            fail_exec_write_once: Arc::new(Mutex::new(false)),
            exec_start_calls: Arc::new(Mutex::new(0)),
            last_patch_request: Arc::new(Mutex::new(None)),
        };

        spawn_named_daemon_on_addr(&self.certs, self.addr, state).await;
    }
}

#[derive(Clone)]
struct StubDaemonState {
    target: String,
    daemon_instance_id: String,
    fail_exec_write_once: Arc<Mutex<bool>>,
    exec_start_calls: Arc<Mutex<usize>>,
    last_patch_request: Arc<Mutex<Option<PatchApplyRequest>>>,
}

pub async fn spawn_broker_with_stub_daemon() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_stub_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: Some(stub_state),
    }
}

pub async fn spawn_broker_with_live_and_dead_targets() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, _stub_state) = spawn_stub_daemon(&certs).await;
    let dead_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{live_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"

[targets.builder-b]
base_url = "https://{dead_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-b"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: None,
    }
}

pub async fn spawn_broker_with_late_target() -> DelayedTargetFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (live_addr, _stub_state) = spawn_stub_daemon(&certs).await;
    let delayed_addr = allocate_addr();
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{live_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"

[targets.builder-b]
base_url = "https://{delayed_addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-b"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    DelayedTargetFixture {
        broker: BrokerFixture {
            _tempdir: tempdir,
            client,
            stub_state: None,
        },
        certs,
        addr: delayed_addr,
    }
}

pub async fn spawn_broker_with_retryable_exec_write_error() -> BrokerFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path());
    let (addr, stub_state) = spawn_retryable_exec_write_daemon(&certs).await;
    let broker_config = tempdir.path().join("broker.toml");
    std::fs::write(
        &broker_config,
        format!(
            r#"[targets.builder-a]
base_url = "https://{addr}"
ca_pem = "{}"
client_cert_pem = "{}"
client_key_pem = "{}"
expected_daemon_name = "builder-a"
"#,
            certs.ca_cert.display(),
            certs.client_cert.display(),
            certs.client_key.display(),
        ),
    )
    .unwrap();

    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_remote-exec-broker"));
    command.arg(&broker_config);
    let transport = TokioChildProcess::new(command).unwrap();
    let client = DummyClientHandler.serve(transport).await.unwrap();

    BrokerFixture {
        _tempdir: tempdir,
        client,
        stub_state: Some(stub_state),
    }
}

async fn exec_start(
    State(state): State<StubDaemonState>,
    Json(_req): Json<ExecStartRequest>,
) -> Json<ExecResponse> {
    *state.exec_start_calls.lock().await += 1;
    Json(ExecResponse {
        daemon_session_id: Some("daemon-session-1".to_string()),
        daemon_instance_id: "daemon-instance-1".to_string(),
        running: true,
        chunk_id: Some("chunk-start".to_string()),
        wall_time_seconds: 0.25,
        exit_code: None,
        original_token_count: Some(1),
        output: "ready".to_string(),
    })
}

async fn patch_apply(
    State(state): State<StubDaemonState>,
    Json(req): Json<PatchApplyRequest>,
) -> Result<Json<PatchApplyResponse>, (StatusCode, Json<RpcErrorBody>)> {
    *state.last_patch_request.lock().await = Some(req.clone());
    if !req.patch.starts_with("*** Begin Patch\n") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: "patch_failed".to_string(),
                message: "invalid patch header".to_string(),
            }),
        ));
    }

    Ok(Json(PatchApplyResponse {
        output: "Success. Updated the following files:\nA hello.txt\n".to_string(),
    }))
}

async fn spawn_stub_daemon(certs: &TestCerts) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, false).await
}

async fn spawn_retryable_exec_write_daemon(
    certs: &TestCerts,
) -> (std::net::SocketAddr, StubDaemonState) {
    spawn_daemon(certs, true).await
}

async fn spawn_daemon(
    certs: &TestCerts,
    fail_exec_write_once: bool,
) -> (std::net::SocketAddr, StubDaemonState) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let state = StubDaemonState {
        target: "builder-a".to_string(),
        daemon_instance_id: "daemon-instance-1".to_string(),
        fail_exec_write_once: Arc::new(Mutex::new(fail_exec_write_once)),
        exec_start_calls: Arc::new(Mutex::new(0)),
        last_patch_request: Arc::new(Mutex::new(None)),
    };

    spawn_named_daemon_on_addr(certs, addr, state.clone()).await;
    (addr, state)
}

async fn spawn_named_daemon_on_addr(
    certs: &TestCerts,
    addr: std::net::SocketAddr,
    state: StubDaemonState,
) {
    let app = Router::new()
        .route("/v1/health", post(health))
        .route("/v1/target-info", post(target_info))
        .route("/v1/exec/start", post(exec_start))
        .route("/v1/exec/write", post(exec_write))
        .route("/v1/patch/apply", post(patch_apply))
        .route("/v1/image/read", post(image_read))
        .with_state(state.clone());

    let daemon_state = remote_exec_daemon::AppState {
        config: Arc::new(remote_exec_daemon::config::DaemonConfig {
            target: state.target.clone(),
            listen: addr,
            default_workdir: PathBuf::from("."),
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: certs.daemon_cert.clone(),
                key_pem: certs.daemon_key.clone(),
                ca_pem: certs.ca_cert.clone(),
            },
        }),
        daemon_instance_id: state.daemon_instance_id.clone(),
        sessions: remote_exec_daemon::exec::store::SessionStore::default(),
    };

    tokio::spawn(async move {
        remote_exec_daemon::tls::serve_tls(app, Arc::new(daemon_state))
            .await
            .unwrap();
    });

    wait_until_ready(certs, addr).await;
}

// crates/remote-exec-broker/tests/mcp_exec.rs
#[tokio::test]
async fn exec_command_intercepts_direct_apply_patch_and_wraps_exec_output() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: hello.txt\n",
        "+hello\n",
        "*** End Patch\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("apply_patch '{patch}'"),
            }),
        )
        .await;

    assert!(result.text_output.contains("Wall time: 0.000 seconds"));
    assert!(result.text_output.contains("Process exited with code 0"));
    assert!(result.text_output.contains("Output:\nSuccess. Updated the following files:"));
    assert!(!result.text_output.contains("Command:"));
    assert!(!result.text_output.contains("Chunk ID:"));
    assert!(result.structured_content["session_id"].is_null());
    assert!(result.structured_content["session_command"].is_null());
    assert_eq!(result.structured_content["wall_time_seconds"], 0.0);
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        patch.to_string()
    );
}

#[tokio::test]
async fn exec_command_intercepts_applypatch_alias_without_allocating_session() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: alias.txt\n",
        "+alias\n",
        "*** End Patch\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": format!("applypatch \"{patch}\""),
            }),
        )
        .await;

    assert!(result.structured_content["session_id"].is_null());
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        patch.to_string()
    );
}

#[tokio::test]
async fn exec_command_non_matching_patch_text_still_uses_exec_start() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let raw_patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: raw.txt\n",
        "+raw\n",
        "*** End Patch\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": raw_patch,
                "tty": true,
                "yield_time_ms": 250
            }),
        )
        .await;

    assert!(result.text_output.contains("Command: *** Begin Patch"));
    assert!(result.structured_content["session_id"].as_str().is_some());
    assert_eq!(fixture.exec_start_calls().await, 1);
    assert!(fixture.last_patch_request().await.is_none());
}

// crates/remote-exec-broker/src/mcp_server.rs
#[test]
fn format_intercepted_patch_text_omits_command_and_chunk_metadata() {
    let text = format_intercepted_patch_text(
        "Success. Updated the following files:\nA hello.txt\n",
    );

    assert!(text.contains("Wall time: 0.000 seconds"));
    assert!(text.contains("Process exited with code 0"));
    assert!(text.contains("Output:\nSuccess. Updated the following files:"));
    assert!(!text.contains("Command:"));
    assert!(!text.contains("Chunk ID:"));
}
```

- [ ] **Step 2: Run the focused broker verification and confirm it fails first**

Run: `cargo test -p remote-exec-broker exec_command_intercepts_direct_apply_patch_and_wraps_exec_output -- --exact --nocapture`
Expected: FAIL because `exec_command` still calls daemon `exec_start`, allocates a live session, and no intercepted formatter exists yet.

- [ ] **Step 3: Add shared patch forwarding, wrapped formatting, and direct interception wiring**

```rust
// crates/remote-exec-broker/src/tools/patch.rs
use remote_exec_proto::public::ApplyPatchInput;
use remote_exec_proto::rpc::PatchApplyRequest;

use crate::mcp_server::ToolCallOutput;

pub async fn forward_patch(
    state: &crate::BrokerState,
    target_name: &str,
    patch: String,
    workdir: Option<String>,
) -> anyhow::Result<String> {
    let target = state.target(target_name)?;
    target.ensure_identity_verified(target_name).await?;
    Ok(
        target
            .client
            .patch_apply(&PatchApplyRequest { patch, workdir })
            .await?
            .output,
    )
}

pub async fn apply_patch(
    state: &crate::BrokerState,
    input: ApplyPatchInput,
) -> anyhow::Result<ToolCallOutput> {
    let output = forward_patch(state, &input.target, input.input, input.workdir).await?;
    Ok(ToolCallOutput::text_and_structured(
        output,
        serde_json::json!({}),
    ))
}

// crates/remote-exec-broker/src/mcp_server.rs
pub fn format_intercepted_patch_text(output: &str) -> String {
    format!("Wall time: 0.000 seconds\nProcess exited with code 0\nOutput:\n{output}")
}

// crates/remote-exec-broker/src/tools/exec.rs
use anyhow::Context;
use remote_exec_proto::public::{CommandToolResult, ExecCommandInput, WriteStdinInput};
use remote_exec_proto::rpc::{ExecStartRequest, ExecWriteRequest};

use super::exec_intercept::maybe_intercept_apply_patch;
use crate::mcp_server::{
    ToolCallOutput, format_command_text, format_intercepted_patch_text, format_poll_text,
};

pub async fn exec_command(
    state: &crate::BrokerState,
    input: ExecCommandInput,
) -> anyhow::Result<ToolCallOutput> {
    if let Some(intercepted) = maybe_intercept_apply_patch(&input.cmd, input.workdir.as_deref()) {
        let output = crate::tools::patch::forward_patch(
            state,
            &input.target,
            intercepted.patch,
            intercepted.workdir,
        )
        .await?;

        return Ok(ToolCallOutput::text_and_structured(
            format_intercepted_patch_text(&output),
            serde_json::to_value(CommandToolResult {
                target: input.target,
                chunk_id: None,
                wall_time_seconds: 0.0,
                exit_code: Some(0),
                session_id: None,
                session_command: None,
                original_token_count: None,
                output,
            })?,
        ));
    }

    let target = state.target(&input.target)?;
    target.ensure_identity_verified(&input.target).await?;
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

    let session_command = input.cmd.clone();
    let session_id = if response.running {
        let daemon_session_id = response
            .daemon_session_id
            .clone()
            .expect("daemon session id");
        Some(
            state
                .sessions
                .insert(
                    input.target.clone(),
                    daemon_session_id,
                    response.daemon_instance_id.clone(),
                    session_command.clone(),
                )
                .await
                .session_id,
        )
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
            session_command: Some(session_command),
            original_token_count: response.original_token_count,
            output: response.output,
        })?,
    ))
}
```

- [ ] **Step 4: Run the focused broker verification again**

Run: `cargo test -p remote-exec-broker exec_command_intercepts_direct_apply_patch_and_wraps_exec_output -- --exact --nocapture`
Expected: PASS, proving explicit direct interception now bypasses daemon `exec_start` and returns wrapped exec-shaped output.

Run: `cargo test -p remote-exec-broker format_intercepted_patch_text_omits_command_and_chunk_metadata -- --exact --nocapture`
Expected: PASS, proving the dedicated formatter omits `Command:` and `Chunk ID:` while preserving the wrapped output surface.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/exec.rs \
        crates/remote-exec-broker/src/tools/patch.rs \
        crates/remote-exec-broker/src/mcp_server.rs \
        crates/remote-exec-broker/tests/mcp_exec.rs \
        crates/remote-exec-broker/tests/support/mod.rs
git commit -m "feat: intercept explicit apply_patch exec commands"
```

### Task 3: Add Heredoc And `cd` Wrapper Interception Coverage

**Files:**
- Modify: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_exec.rs:1-226`
- Test/Verify: `cargo test -p remote-exec-broker exec_command_intercepts_applypatch_heredoc_with_cd_wrapper -- --exact --nocapture`

**Testing approach:** `TDD`
Reason: heredoc extraction and `cd <path> && ...` resolution are the remaining documented edge cases, and they are easiest to pin down with parser unit tests plus broker integration tests that inspect forwarded patch/workdir payloads.

- [ ] **Step 1: Add failing heredoc, `cd`, and invalid-intercept regression tests**

```rust
// crates/remote-exec-broker/src/tools/exec_intercept.rs
#[cfg(test)]
mod tests {
    use super::{InterceptedApplyPatch, maybe_intercept_apply_patch};

    #[test]
    fn parses_applypatch_heredoc_with_cd_wrapper_relative_to_workdir() {
        let cmd = concat!(
            "cd nested && applypatch <<'PATCH'\n",
            "*** Begin Patch\n",
            "*** Add File: hello.txt\n",
            "+hello\n",
            "*** End Patch\n",
            "PATCH\n",
        );

        assert_eq!(
            maybe_intercept_apply_patch(cmd, Some("outer")),
            Some(InterceptedApplyPatch {
                patch: concat!(
                    "*** Begin Patch\n",
                    "*** Add File: hello.txt\n",
                    "+hello\n",
                    "*** End Patch\n",
                )
                .to_string(),
                workdir: Some("outer/nested".to_string()),
            })
        );
    }
}

// crates/remote-exec-broker/tests/mcp_exec.rs
#[tokio::test]
async fn exec_command_intercepts_applypatch_heredoc_with_cd_wrapper() {
    let fixture = support::spawn_broker_with_stub_daemon().await;
    let patch = concat!(
        "*** Begin Patch\n",
        "*** Add File: hello.txt\n",
        "+hello\n",
        "*** End Patch\n",
    );
    let cmd = concat!(
        "cd nested && applypatch <<'PATCH'\n",
        "*** Begin Patch\n",
        "*** Add File: hello.txt\n",
        "+hello\n",
        "*** End Patch\n",
        "PATCH\n",
    );

    let result = fixture
        .call_tool(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": cmd,
                "workdir": "outer"
            }),
        )
        .await;

    assert!(result.text_output.contains("Output:\nSuccess. Updated the following files:"));
    assert_eq!(fixture.exec_start_calls().await, 0);
    let forwarded = fixture.last_patch_request().await.unwrap();
    assert_eq!(forwarded.patch, patch.to_string());
    assert_eq!(forwarded.workdir, Some("outer/nested".to_string()));
}

#[tokio::test]
async fn exec_command_invalid_intercepted_patch_surfaces_tool_error() {
    let fixture = support::spawn_broker_with_stub_daemon().await;

    let error = fixture
        .call_tool_error(
            "exec_command",
            serde_json::json!({
                "target": "builder-a",
                "cmd": "apply_patch 'not a patch'"
            }),
        )
        .await;

    assert!(error.contains("patch_failed") || error.contains("invalid patch"));
    assert_eq!(fixture.exec_start_calls().await, 0);
    assert_eq!(
        fixture.last_patch_request().await.unwrap().patch,
        "not a patch".to_string()
    );
}
```

- [ ] **Step 2: Run the focused heredoc verification and confirm it fails first**

Run: `cargo test -p remote-exec-broker exec_command_intercepts_applypatch_heredoc_with_cd_wrapper -- --exact --nocapture`
Expected: FAIL because the parser from Task 1 only recognizes direct quoted forms and does not yet extract heredoc bodies or `cd <path> && ...` workdirs.

- [ ] **Step 3: Extend the parser to handle documented heredoc and `cd` wrappers**

```rust
// crates/remote-exec-broker/src/tools/exec_intercept.rs
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterceptedApplyPatch {
    pub patch: String,
    pub workdir: Option<String>,
}

pub fn maybe_intercept_apply_patch(
    cmd: &str,
    workdir: Option<&str>,
) -> Option<InterceptedApplyPatch> {
    let trimmed = cmd.trim();
    if let Some(patch) = parse_direct_invocation(trimmed) {
        return Some(InterceptedApplyPatch {
            patch,
            workdir: workdir.map(ToString::to_string),
        });
    }

    let (effective_workdir, script) = split_cd_wrapper(trimmed, workdir)?;
    let (command_name, body) = parse_heredoc_invocation(script)?;
    if command_name != "apply_patch" && command_name != "applypatch" {
        return None;
    }

    Some(InterceptedApplyPatch {
        patch: format!("{body}\n"),
        workdir: effective_workdir,
    })
}

fn parse_direct_invocation(cmd: &str) -> Option<String> {
    ["apply_patch", "applypatch"]
        .into_iter()
        .find_map(|name| {
            let rest = cmd.strip_prefix(name)?.trim_start();
            parse_single_quoted_argument(rest)
        })
}

fn parse_single_quoted_argument(rest: &str) -> Option<String> {
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }

    let end = rest[1..].find(quote)? + 1;
    let patch = &rest[1..end];
    let trailing = &rest[end + 1..];
    if !trailing.trim().is_empty() {
        return None;
    }

    Some(patch.to_string())
}

fn split_cd_wrapper<'a>(
    cmd: &'a str,
    workdir: Option<&str>,
) -> Option<(Option<String>, &'a str)> {
    if let Some(rest) = cmd.strip_prefix("cd ") {
        let (path, tail) = rest.split_once("&&")?;
        let path = path.trim();
        if path.is_empty() || path.chars().any(char::is_whitespace) {
            return None;
        }

        let mut resolved = workdir.map(PathBuf::from).unwrap_or_default();
        resolved.push(path);
        return Some((Some(resolved.display().to_string()), tail.trim_start()));
    }

    Some((workdir.map(ToString::to_string), cmd))
}

fn parse_heredoc_invocation(cmd: &str) -> Option<(&str, &str)> {
    let (head, rest) = cmd.split_once("<<'")?;
    let command_name = head.trim();
    let (delimiter, body_with_newline) = rest.split_once("'\n")?;
    let marker = format!("\n{delimiter}");
    let (body, trailing) = body_with_newline.rsplit_once(&marker)?;
    if !trailing.trim().is_empty() {
        return None;
    }
    Some((command_name, body))
}
```

- [ ] **Step 4: Run the full verification for the interception batch**

Run: `cargo test -p remote-exec-broker --test mcp_exec -- --nocapture`
Expected: PASS for direct interception, alias interception, heredoc/cd interception, invalid intercepted failure, and non-match fallback coverage.

Run: `cargo fmt --all --check`
Expected: PASS with no formatting diffs.

Run: `cargo test --workspace`
Expected: PASS across broker, daemon, proto, admin, PKI, and end-to-end tests.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-broker/src/tools/exec_intercept.rs \
        crates/remote-exec-broker/tests/mcp_exec.rs
git commit -m "feat: support apply_patch exec heredoc interception"
```
