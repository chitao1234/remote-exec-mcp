# Phase B Refactor Holdouts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Finish Phase B items `#13` through `#25` from `docs/CODE_AUDIT_ROUND2.md` without mixing them into correctness/security fixes or later operational polish.

**Architecture:** Treat the audit as stale review input and validate each finding against the current tree before changing code. Execute the work as small refactor commits: first low-risk holdouts, then protocol/API typing, then C++ cleanup. Do not bundle the typed-ID and exec-response contract changes with unrelated mechanical cleanups.

**Tech Stack:** Rust 2024 workspace with Serde, Schemars, Tokio, reqwest/rmcp; C++11 daemon with POSIX and Windows XP-compatible builds; existing Cargo integration tests and C++ make targets.

---

## Scope

Included audit items: `#13` through `#25` from `docs/CODE_AUDIT_ROUND2.md`.

Excluded from this plan: Phase A items `#1` through `#12`, Phase C structural/operational items `#26+`, and any new behavior outside the listed refactor holdouts.

## Current Validation Snapshot

- `#13` is stale as a production finding. The remaining broker `"port_tunnel_limit_exceeded"` literals are in `#[cfg(test)]` fixtures in `tcp_bridge.rs` and `udp_bridge.rs`, not production logic. Still clean them up because tests should exercise typed codes too.
- `#14` remains valid. `TransferCompression` still lives in `remote-exec-proto/src/rpc.rs`, and `transfer.rs` reaches back to it with `crate::rpc::TransferCompression`.
- `#15` remains valid. `remote-exec-host/src/exec/support.rs` and `remote-exec-host/src/port_forward/error.rs` both define local `rpc_error` wrappers with identical logging and bad-request construction.
- `#16` and `#17` remain valid. Broker port-forward heartbeat timing is still module-level constants plus `#[cfg(debug_assertions)]` environment overrides. Host has a `PortTunnelTimings` accessor, but it currently only covers `resume_timeout`.
- `#18` remains valid. Broker streamable HTTP SSE config still uses `Option<u64>` with `Some(0)` meaning disabled after deserialization.
- `#19` remains valid but needs a compatibility-preserving rollout. `TargetInfoResponse`, `CachedDaemonInfo`, and public `ListTargetDaemonInfo` still use bare `u32` for `port_forward_protocol_version`, with default `0`.
- `#20` remains valid but broad. `ExecResponse.daemon_session_id: Option<String>` is still the shared start/write response shape, and broker-side validation repairs the invariant after decoding.
- `#21` remains valid. ID generation is in `remote-exec-host/src/ids.rs`, not proto, and it still returns bare `String`.
- `#22` remains valid. `remote-exec-proto/src/port_forward.rs` still returns `anyhow::Result`.
- `#23` remains valid. `platform::join_path` and `path_utils::join_path` both exist in C++; `shell_policy.cpp` still calls `platform::join_path`.
- `#24` remains valid. `patch_engine.cpp` still carries a private `make_directory_if_missing` and `create_parent_directories` implementation while transfer has shared helpers.
- `#25` remains valid. C++ `DaemonConfig.port_forward_max_worker_threads` is redundant and only tests read it.

## File Structure

- `docs/superpowers/plans/2026-05-11-phase-b-refactor-holdouts.md`: this plan.
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`, `udp_bridge.rs`: test fixture code literals for `RpcErrorCode::PortTunnelLimitExceeded`.
- `crates/remote-exec-proto/src/transfer.rs`, `rpc.rs`: move `TransferCompression` and header parsing imports.
- Rust transfer call sites in broker, daemon, and host: import `TransferCompression` from `remote_exec_proto::transfer`.
- `crates/remote-exec-host/src/error.rs`, `exec/support.rs`, `port_forward/error.rs`: consolidate host RPC error logging helper.
- `crates/remote-exec-broker/src/port_forward/timings.rs` and `mod.rs`, plus tests that depend on heartbeat timing: mirror the host timing pattern and remove debug-env overrides.
- `crates/remote-exec-broker/src/config.rs`, `mcp_server.rs`, and config tests: replace SSE millisecond sentinel fields with an explicit deserialized interval type.
- `crates/remote-exec-proto/src/rpc.rs`, `public.rs`, broker `target/handle.rs`, `state.rs`, `tools/targets.rs`, and daemon/host target-info production: tighten port-forward protocol version typing.
- `crates/remote-exec-proto/src/rpc.rs`: split exec start/write response contracts in place to avoid a file split during this phase.
- `crates/remote-exec-host/src/ids.rs`, broker/host stores and port-forward tunnel metadata: add typed ID newtypes in the smallest viable layer.
- `crates/remote-exec-proto/src/port_forward.rs`: add `PortForwardProtoError`.
- C++ daemon files: `include/platform.h`, `src/platform.cpp`, `src/shell_policy.cpp`, `include/path_utils.h`, `src/path_utils.cpp`, `src/patch_engine.cpp`, `src/transfer_ops_internal.h`, `src/transfer_ops_fs.cpp`, `include/config.h`, `src/config.cpp`, and tests.

---

### Task 1: Save The Phase B Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-b-refactor-holdouts.md`
- Test/Verify: `test -f docs/superpowers/plans/2026-05-11-phase-b-refactor-holdouts.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked plan artifact only.

- [ ] **Step 1: Verify this plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-b-refactor-holdouts.md`
Expected: command exits successfully.

- [ ] **Step 2: Review the heading and scope.**

Run: `sed -n '1,90p' docs/superpowers/plans/2026-05-11-phase-b-refactor-holdouts.md`
Expected: output names Phase B only, includes the required agentic-worker header, and explicitly includes items `#13` through `#25`.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-b-refactor-holdouts.md
git commit -m "docs: plan phase b refactor holdouts"
```

### Task 2: Replace Remaining Broker Test Error-Code Literals

**Finding:** `#13`

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker port_forward::tcp_bridge::tests::tcp_listener_pressure_error_counts_drop_without_failing_forward`
  - `cargo test -p remote-exec-broker port_forward::tcp_bridge::tests::tcp_listener_forward_drop_counts_drop_without_failing_forward`
  - `cargo test -p remote-exec-broker port_forward::udp_bridge::tests::udp_listener_pressure_error_counts_drop_without_failing_forward`
  - `cargo test -p remote-exec-broker port_forward::udp_bridge::tests::udp_listener_forward_drop_counts_drop_without_failing_forward`

**Testing approach:** existing tests + targeted verification
Reason: These are test fixtures, not production behavior; the correct verification is that the same tests compile and pass while using typed wire values.

- [ ] **Step 1: Add typed-code imports in the test modules.**

In both `tcp_bridge.rs` and `udp_bridge.rs` test modules, extend imports with:

```rust
use remote_exec_proto::rpc::RpcErrorCode;
```

- [ ] **Step 2: Replace JSON fixture literals.**

Change every test JSON value:

```rust
"code": "port_tunnel_limit_exceeded",
```

to:

```rust
"code": RpcErrorCode::PortTunnelLimitExceeded.wire_value(),
```

- [ ] **Step 3: Replace `ForwardDropMeta.reason` fixture literals.**

Change:

```rust
reason: "port_tunnel_limit_exceeded".to_string(),
```

to:

```rust
reason: RpcErrorCode::PortTunnelLimitExceeded.wire_value().to_string(),
```

- [ ] **Step 4: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-broker port_forward::tcp_bridge::tests::tcp_listener_pressure_error_counts_drop_without_failing_forward
cargo test -p remote-exec-broker port_forward::tcp_bridge::tests::tcp_listener_forward_drop_counts_drop_without_failing_forward
cargo test -p remote-exec-broker port_forward::udp_bridge::tests::udp_listener_pressure_error_counts_drop_without_failing_forward
cargo test -p remote-exec-broker port_forward::udp_bridge::tests::udp_listener_forward_drop_counts_drop_without_failing_forward
```

Expected: all four tests pass.

- [ ] **Step 5: Confirm no broker production holdout remains.**

Run: `rg -n '"port_tunnel_limit_exceeded"' crates/remote-exec-broker/src/port_forward`
Expected: no output from broker port-forward source. If output remains only in comments, remove or justify it before committing.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/src/port_forward/udp_bridge.rs
git commit -m "test: use typed port tunnel limit code fixtures"
```

### Task 3: Move TransferCompression Into Transfer Module

**Finding:** `#14`

**Files:**
- Modify: `crates/remote-exec-proto/src/transfer.rs`
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify imports in:
  - `crates/remote-exec-broker/src/tools/transfer/*.rs`
  - `crates/remote-exec-broker/src/local_transfer.rs`
  - `crates/remote-exec-host/src/transfer/**/*.rs`
  - `crates/remote-exec-daemon/src/transfer/*.rs`
  - daemon/broker tests that import `remote_exec_proto::rpc::TransferCompression`
- Test/Verify:
  - `cargo test -p remote-exec-proto transfer_header`
  - `cargo test -p remote-exec-broker --test mcp_transfer`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`

**Testing approach:** characterization/integration test
Reason: This is a type-location refactor. The wire format must remain unchanged, so existing serialization/header tests are the best guard.

- [ ] **Step 1: Move the enum definition to `transfer.rs`.**

Insert this block in `crates/remote-exec-proto/src/transfer.rs` before `TransferExportRequest`:

```rust
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransferCompression {
    #[default]
    None,
    Zstd,
}

impl TransferCompression {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn wire_value(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Zstd => "zstd",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "zstd" => Some(Self::Zstd),
            _ => None,
        }
    }
}
```

- [ ] **Step 2: Update transfer DTO field types.**

In `transfer.rs`, replace every `crate::rpc::TransferCompression` with `TransferCompression`, including `skip_serializing_if`:

```rust
#[serde(default, skip_serializing_if = "TransferCompression::is_none")]
pub compression: TransferCompression,
```

- [ ] **Step 3: Delete the enum definition from `rpc.rs` and re-export the transfer type.**

Remove `pub enum TransferCompression` and its `impl` from `rpc.rs`. Update the existing public re-export block at the top of `rpc.rs` to include `TransferCompression`; this single `pub use` both preserves the old `remote_exec_proto::rpc::TransferCompression` compatibility path and keeps the name in local scope for header parsing code:

```rust
pub use crate::transfer::{
    TransferCompression, TransferExportMetadata, TransferExportRequest, TransferImportMetadata,
    TransferImportRequest, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};
```

- [ ] **Step 4: Prefer new import paths in call sites.**

Change imports such as:

```rust
use remote_exec_proto::rpc::TransferCompression;
```

to:

```rust
use remote_exec_proto::transfer::TransferCompression;
```

For grouped imports that also import RPC request/response types, split them:

```rust
use remote_exec_proto::rpc::{TransferExportRequest, TransferImportRequest};
use remote_exec_proto::transfer::TransferCompression;
```

- [ ] **Step 5: Verify the old back-edge is gone.**

Run: `rg -n 'crate::rpc::TransferCompression|rpc::TransferCompression' crates/remote-exec-proto/src crates/remote-exec-broker/src crates/remote-exec-host/src crates/remote-exec-daemon/src`
Expected: no output. The compatibility re-export must be the grouped `pub use crate::transfer::{ TransferCompression, ... }` in `rpc.rs`; no module should reach from `transfer.rs` back into `rpc.rs`.

- [ ] **Step 6: Run transfer tests.**

Run:

```bash
cargo test -p remote-exec-proto transfer_header
cargo test -p remote-exec-broker --test mcp_transfer
cargo test -p remote-exec-daemon --test transfer_rpc
```

Expected: all pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-proto/src/transfer.rs crates/remote-exec-proto/src/rpc.rs crates/remote-exec-broker/src crates/remote-exec-host/src crates/remote-exec-daemon/src crates/remote-exec-broker/tests crates/remote-exec-daemon/tests
git commit -m "refactor: move transfer compression to transfer proto"
```

### Task 4: Consolidate Host RPC Error Constructors

**Finding:** `#15`

**Files:**
- Modify: `crates/remote-exec-host/src/error.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/port_forward/error.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host`
  - `cargo test -p remote-exec-broker --test mcp_exec`

**Testing approach:** existing tests + targeted verification
Reason: This is constructor deduplication. Behavior should stay byte-for-byte equivalent for error code/status/message.

- [ ] **Step 1: Add the logged bad-request helper in `error.rs`.**

Add this function below `bad_request`:

```rust
pub(crate) fn logged_bad_request(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
    bad_request(code, message)
}
```

- [ ] **Step 2: Remove the exec-local duplicate.**

Delete `pub fn rpc_error(...)` from `exec/support.rs`. Change call sites in `crates/remote-exec-host/src/exec` from:

```rust
support::rpc_error(code, message)
```

or imported `rpc_error(...)` to:

```rust
crate::error::logged_bad_request(code, message)
```

- [ ] **Step 3: Remove the port-forward duplicate and keep the local alias.**

Delete `pub(super) fn rpc_error(...)` from `port_forward/error.rs`. Add this replacement so existing port-forward modules keep their local import style while sharing the single implementation:

```rust
pub(super) use crate::error::logged_bad_request as rpc_error;
```

- [ ] **Step 4: Verify no duplicate body remains.**

Run: `rg -n 'fn rpc_error|logged_bad_request' crates/remote-exec-host/src`
Expected: only `error.rs` defines the logging constructor; `port_forward/error.rs` may re-export it but must not duplicate the function body.

- [ ] **Step 5: Run tests.**

Run:

```bash
cargo test -p remote-exec-host
cargo test -p remote-exec-broker --test mcp_exec
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src/error.rs crates/remote-exec-host/src/exec crates/remote-exec-host/src/port_forward
git commit -m "refactor: centralize host rpc error construction"
```

### Task 5: Mirror PortTunnelTimings In Broker And Remove Debug Env Overrides

**Findings:** `#16`, `#17`

**Files:**
- Create: `crates/remote-exec-broker/src/port_forward/timings.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Modify test support that sets `REMOTE_EXEC_TEST_PORT_TUNNEL_HEARTBEAT_*`, currently `crates/remote-exec-broker/tests/support/spawners.rs`
- Modify heartbeat-sensitive integration tests in `crates/remote-exec-broker/tests/mcp_forward_ports.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker port_forward::tunnel`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_recovers_idle_connect_tunnel_after_heartbeat_timeout`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_reports_reconnecting_until_connect_side_is_ready`

**Testing approach:** characterization/integration test
Reason: The change should preserve test timing and production timing while removing the environment-variable escape hatch from debug builds.

- [ ] **Step 1: Create broker timing module.**

Create `crates/remote-exec-broker/src/port_forward/timings.rs`:

```rust
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub(crate) struct PortTunnelTimings {
    pub(crate) heartbeat_interval: Duration,
    pub(crate) heartbeat_timeout: Duration,
}

impl PortTunnelTimings {
    #[cfg(not(test))]
    pub(crate) fn production() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(10),
            heartbeat_timeout: Duration::from_secs(30),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            heartbeat_interval: Duration::from_millis(25),
            heartbeat_timeout: Duration::from_millis(250),
        }
    }
}

pub(crate) fn timings() -> PortTunnelTimings {
    #[cfg(test)]
    {
        PortTunnelTimings::for_test()
    }
    #[cfg(not(test))]
    {
        PortTunnelTimings::production()
    }
}
```

- [ ] **Step 2: Wire the module in `mod.rs`.**

Add:

```rust
mod timings;
```

Remove these broker constants and helpers from `mod.rs`:

```rust
PORT_TUNNEL_HEARTBEAT_INTERVAL
PORT_TUNNEL_HEARTBEAT_TIMEOUT
port_tunnel_heartbeat_interval()
port_tunnel_heartbeat_timeout()
test_duration_override()
```

- [ ] **Step 3: Update tunnel heartbeat setup.**

In `tunnel.rs`, replace:

```rust
use super::{port_tunnel_heartbeat_interval, port_tunnel_heartbeat_timeout};
```

with:

```rust
use super::timings::timings;
```

Replace:

```rust
let heartbeat_interval = port_tunnel_heartbeat_interval();
let heartbeat_timeout = port_tunnel_heartbeat_timeout();
```

with:

```rust
let timing = timings();
let heartbeat_interval = timing.heartbeat_interval;
let heartbeat_timeout = timing.heartbeat_timeout;
```

- [ ] **Step 4: Remove debug-env setup from integration spawners.**

In `crates/remote-exec-broker/tests/support/spawners.rs`, delete these helpers entirely:

```rust
spawn_broker_with_stub_port_forward_version_and_fast_heartbeat
spawn_broker_with_stub_port_forward_version_and_heartbeat
spawn_broker_with_stub_port_forward_version_and_env
spawn_broker_with_local_and_stub_port_forward_version_and_fast_heartbeat
spawn_broker_with_local_and_stub_port_forward_version_and_heartbeat
spawn_broker_with_local_and_stub_port_forward_version_and_extra_config_and_env
```

Replace their uses with the existing non-env helpers:

```rust
spawn_broker_with_stub_port_forward_version(4)
spawn_broker_with_local_and_stub_port_forward_version(4)
spawn_broker_with_local_and_stub_port_forward_version_and_extra_config(4, extra_config)
```

Keep `spawn_broker_child_with_env`; it is the shared lower-level child process helper. It should no longer receive `REMOTE_EXEC_TEST_PORT_TUNNEL_HEARTBEAT_*` values from any port-forward spawner.

- [ ] **Step 5: Keep heartbeat-timeout coverage out of spawned-binary env overrides.**

In `crates/remote-exec-broker/tests/mcp_forward_ports.rs`, remove fast-heartbeat spawner calls. The only test that specifically needs heartbeat timing is `forward_ports_recovers_idle_connect_tunnel_after_heartbeat_timeout`; keep it deterministic by triggering the reconnect with the existing stub control:

```rust
support::stub_daemon::force_close_connect_port_tunnel_transport(&fixture.stub_state).await;
```

Do not wait for a production 30-second heartbeat timeout in integration tests. The unit test `port_forward::tunnel::tests::heartbeat_timeout_surfaces_retryable_transport_error` continues to cover heartbeat timeout behavior with `#[cfg(test)]` fast timings.

- [ ] **Step 6: Verify no test env escape hatch remains.**

Run: `rg -n 'REMOTE_EXEC_TEST_PORT_TUNNEL|test_duration_override|debug_assertions' crates/remote-exec-broker/src crates/remote-exec-broker/tests`
Expected: no output.

- [ ] **Step 7: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-broker port_forward::tunnel
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_recovers_idle_connect_tunnel_after_heartbeat_timeout
cargo test -p remote-exec-broker --test mcp_forward_ports forward_ports_reports_reconnecting_until_connect_side_is_ready
```

Expected: all pass.

- [ ] **Step 8: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward crates/remote-exec-broker/tests/support/spawners.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "refactor: centralize broker port tunnel timings"
```

### Task 6: Replace SSE Millisecond Sentinel With Typed Config

**Finding:** `#18`

**Files:**
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `crates/remote-exec-broker/src/mcp_server.rs`
- Test/Verify:
  - `cargo test -p remote-exec-broker config::tests::load_accepts_streamable_http_mcp_config`
  - `cargo test -p remote-exec-broker config::tests`

**Testing approach:** TDD
Reason: The bug is config semantics. Tests should prove `0` becomes a disabled interval during deserialization, not later in server startup.

- [ ] **Step 1: Add a failing config assertion.**

In `load_accepts_streamable_http_mcp_config`, change the expected values from:

```rust
assert_eq!(sse_keep_alive_ms, Some(0));
assert_eq!(sse_retry_ms, Some(1000));
```

to:

```rust
assert_eq!(sse_keep_alive, SseInterval::Disabled);
assert_eq!(sse_retry, SseInterval::Duration(std::time::Duration::from_millis(1000)));
```

Also update the pattern names in that match arm to `sse_keep_alive` and `sse_retry`.

- [ ] **Step 2: Run the failing focused test.**

Run: `cargo test -p remote-exec-broker config::tests::load_accepts_streamable_http_mcp_config`
Expected: compile fails because `SseInterval` and the renamed fields do not exist.

- [ ] **Step 3: Add `SseInterval` and custom deserializer.**

In `config.rs`, add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseInterval {
    Disabled,
    Duration(std::time::Duration),
}

impl SseInterval {
    pub(crate) fn as_duration(self) -> Option<std::time::Duration> {
        match self {
            Self::Disabled => None,
            Self::Duration(duration) => Some(duration),
        }
    }
}

fn deserialize_sse_interval<'de, D>(deserializer: D) -> Result<SseInterval, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let millis = u64::deserialize(deserializer)?;
    Ok(if millis == 0 {
        SseInterval::Disabled
    } else {
        SseInterval::Duration(std::time::Duration::from_millis(millis))
    })
}

fn default_streamable_http_sse_keep_alive() -> SseInterval {
    SseInterval::Duration(std::time::Duration::from_millis(15_000))
}

fn default_streamable_http_sse_retry() -> SseInterval {
    SseInterval::Duration(std::time::Duration::from_millis(3_000))
}
```

- [ ] **Step 4: Rename streamable HTTP config fields.**

Change the enum variant fields from:

```rust
#[serde(default = "default_streamable_http_sse_keep_alive_ms")]
sse_keep_alive_ms: Option<u64>,
#[serde(default = "default_streamable_http_sse_retry_ms")]
sse_retry_ms: Option<u64>,
```

to:

```rust
#[serde(
    default = "default_streamable_http_sse_keep_alive",
    rename = "sse_keep_alive_ms",
    deserialize_with = "deserialize_sse_interval"
)]
sse_keep_alive: SseInterval,
#[serde(
    default = "default_streamable_http_sse_retry",
    rename = "sse_retry_ms",
    deserialize_with = "deserialize_sse_interval"
)]
sse_retry: SseInterval,
```

Delete `default_streamable_http_sse_keep_alive_ms`, `default_streamable_http_sse_retry_ms`, and `duration_from_millis`.

- [ ] **Step 5: Update MCP server startup.**

In `mcp_server.rs`, match the renamed fields and call:

```rust
serve_streamable_http(
    state,
    *listen,
    path,
    *stateful,
    sse_keep_alive.as_duration(),
    sse_retry.as_duration(),
)
.await
```

- [ ] **Step 6: Run config tests.**

Run:

```bash
cargo test -p remote-exec-broker config::tests::load_accepts_streamable_http_mcp_config
cargo test -p remote-exec-broker config::tests
```

Expected: all pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-broker/src/config.rs crates/remote-exec-broker/src/mcp_server.rs
git commit -m "refactor: type streamable http sse intervals"
```

### Task 7: Tighten Port-Forward Protocol Version Typing

**Finding:** `#19`

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-proto/src/public.rs`
- Modify: `crates/remote-exec-proto/src/port_tunnel.rs`
- Modify: `crates/remote-exec-host/src/state.rs`
- Modify: `crates/remote-exec-broker/src/target/handle.rs`
- Modify: `crates/remote-exec-broker/src/state.rs`
- Modify: `crates/remote-exec-broker/src/tools/targets.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto`
  - `cargo test -p remote-exec-broker --test mcp_assets`
  - `cargo test -p remote-exec-broker --test multi_target -- --nocapture`

**Testing approach:** characterization/integration test
Reason: This changes public and internal DTO shape while preserving old-daemon compatibility. Tests must cover missing JSON field and v4 reporting.

- [ ] **Step 1: Add typed protocol version newtype in `rpc.rs`.**

Add:

```rust
use std::num::NonZeroU32;

#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize, schemars::JsonSchema, PartialEq, Eq, PartialOrd, Ord)]
#[serde(transparent)]
pub struct PortForwardProtocolVersion(NonZeroU32);

impl PortForwardProtocolVersion {
    pub fn v4() -> Self {
        Self(NonZeroU32::new(4).expect("v4 is nonzero"))
    }

    pub fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}
```

- [ ] **Step 2: Change internal target info to `Option<PortForwardProtocolVersion>`.**

In `TargetInfoResponse`, change:

```rust
#[serde(default)]
pub port_forward_protocol_version: u32,
```

to:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub port_forward_protocol_version: Option<PortForwardProtocolVersion>,
```

Keep `supports_port_forward: bool` for compatibility with existing JSON.

- [ ] **Step 3: Change public list target result similarly.**

In `public.rs`, change `ListTargetDaemonInfo.port_forward_protocol_version` to:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub port_forward_protocol_version: Option<PortForwardProtocolVersion>,
```

Import the newtype with:

```rust
use crate::rpc::{ExecWarning, PortForwardProtocolVersion, TransferWarning};
```

- [ ] **Step 4: Update producers.**

In `remote-exec-host/src/state.rs`, set:

```rust
port_forward_protocol_version: Some(PortForwardProtocolVersion::v4()),
```

In C++ daemon JSON, keep `"port_forward_protocol_version": 4`; Rust deserialization will parse it into `Some`.

- [ ] **Step 5: Update broker cache and feature checks.**

In `CachedDaemonInfo`, change the field to `Option<PortForwardProtocolVersion>`.

Update the v4 support check in `broker/src/state.rs` from:

```rust
info.supports_port_forward && info.port_forward_protocol_version >= 4
```

to:

```rust
info.supports_port_forward
    && info
        .port_forward_protocol_version
        .is_some_and(|version| version.get() >= 4)
```

In `tools/targets.rs`, display `forward_protocol=v{version}` only when `Some(version)`.

- [ ] **Step 6: Preserve missing-field compatibility test.**

Update `remote-exec-proto/src/port_tunnel.rs` test that currently expects `0`:

```rust
assert_eq!(info.port_forward_protocol_version, None);
```

Add a v4 parse assertion:

```rust
let info: TargetInfoResponse = serde_json::from_value(serde_json::json!({
    "target": "daemon",
    "daemon_version": "0.1.0",
    "daemon_instance_id": "inst",
    "hostname": "host",
    "platform": "linux",
    "arch": "x86_64",
    "supports_pty": true,
    "supports_image_read": true,
    "supports_port_forward": true,
    "port_forward_protocol_version": 4
})).unwrap();
assert_eq!(info.port_forward_protocol_version.map(|version| version.get()), Some(4));
```

- [ ] **Step 7: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-proto
cargo test -p remote-exec-broker --test mcp_assets
cargo test -p remote-exec-broker --test multi_target -- --nocapture
```

Expected: all pass.

- [ ] **Step 8: Commit.**

```bash
git add crates/remote-exec-proto/src crates/remote-exec-host/src/state.rs crates/remote-exec-broker/src
git commit -m "refactor: type port forward protocol versions"
```

### Task 8: Split Exec Start And Completed Response Contracts

**Finding:** `#20`

**Files:**
- Modify: `crates/remote-exec-proto/src/rpc.rs`
- Modify: `crates/remote-exec-host/src/exec/support.rs`
- Modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Modify: `crates/remote-exec-daemon/src/exec/mod.rs`
- Modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Modify exec tests in broker and daemon.
- Test/Verify:
  - `cargo test -p remote-exec-proto exec_response`
  - `cargo test -p remote-exec-daemon --test exec_rpc`
  - `cargo test -p remote-exec-broker --test mcp_exec`

**Testing approach:** TDD for proto serialization plus integration tests
Reason: This is a wire-contract tightening. The proto test should fail first, then daemon/broker integration tests prove compatibility within this workspace.

- [ ] **Step 1: Add response structs in `rpc.rs`.**

Introduce these public response structs:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecOutputResponse {
    pub daemon_instance_id: String,
    pub running: bool,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub original_token_count: Option<u32>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ExecWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecRunningResponse {
    pub daemon_session_id: String,
    pub output: ExecOutputResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecCompletedResponse {
    pub output: ExecOutputResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecResponse {
    Running(ExecRunningResponse),
    Completed(ExecCompletedResponse),
}
```

Add a private wire struct and custom serde impl so field names stay identical on the wire while malformed combinations are rejected during deserialization:

```rust
#[derive(Debug, Serialize, Deserialize)]
struct ExecResponseWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daemon_session_id: Option<String>,
    daemon_instance_id: String,
    running: bool,
    chunk_id: Option<String>,
    wall_time_seconds: f64,
    exit_code: Option<i32>,
    original_token_count: Option<u32>,
    output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<ExecWarning>,
}

impl ExecResponseWire {
    fn output(self) -> ExecOutputResponse {
        ExecOutputResponse {
            daemon_instance_id: self.daemon_instance_id,
            running: self.running,
            chunk_id: self.chunk_id,
            wall_time_seconds: self.wall_time_seconds,
            exit_code: self.exit_code,
            original_token_count: self.original_token_count,
            output: self.output,
            warnings: self.warnings,
        }
    }
}

impl From<ExecResponse> for ExecResponseWire {
    fn from(response: ExecResponse) -> Self {
        match response {
            ExecResponse::Running(response) => {
                let output = response.output;
                Self {
                    daemon_session_id: Some(response.daemon_session_id),
                    daemon_instance_id: output.daemon_instance_id,
                    running: true,
                    chunk_id: output.chunk_id,
                    wall_time_seconds: output.wall_time_seconds,
                    exit_code: output.exit_code,
                    original_token_count: output.original_token_count,
                    output: output.output,
                    warnings: output.warnings,
                }
            }
            ExecResponse::Completed(response) => {
                let output = response.output;
                Self {
                    daemon_session_id: None,
                    daemon_instance_id: output.daemon_instance_id,
                    running: false,
                    chunk_id: output.chunk_id,
                    wall_time_seconds: output.wall_time_seconds,
                    exit_code: output.exit_code,
                    original_token_count: output.original_token_count,
                    output: output.output,
                    warnings: output.warnings,
                }
            }
        }
    }
}

impl serde::Serialize for ExecResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ExecResponseWire::from(self.clone()).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for ExecResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ExecResponseWire::deserialize(deserializer)?;
        let running = wire.running;
        let daemon_session_id = wire.daemon_session_id.clone();
        match (running, daemon_session_id) {
            (true, Some(daemon_session_id)) => Ok(Self::Running(ExecRunningResponse {
                daemon_session_id,
                output: wire.output(),
            })),
            (true, None) => Err(serde::de::Error::custom(
                "running exec response missing daemon_session_id",
            )),
            (false, None) => Ok(Self::Completed(ExecCompletedResponse {
                output: wire.output(),
            })),
            (false, Some(_)) => Err(serde::de::Error::custom(
                "completed exec response unexpectedly included daemon_session_id",
            )),
        }
    }
}
```

Keep field names identical on the wire: running responses serialize `daemon_session_id`; completed responses do not.

- [ ] **Step 2: Add failing proto tests.**

Add tests named:

```rust
#[test]
fn running_exec_response_requires_daemon_session_id() {
    let value = serde_json::json!({
        "daemon_instance_id": "inst",
        "running": true,
        "chunk_id": "chunk",
        "wall_time_seconds": 0.1,
        "exit_code": null,
        "original_token_count": 1,
        "output": "hi"
    });
    assert!(serde_json::from_value::<ExecResponse>(value).is_err());
}

#[test]
fn completed_exec_response_rejects_daemon_session_id() {
    let value = serde_json::json!({
        "daemon_session_id": "daemon-session-1",
        "daemon_instance_id": "inst",
        "running": false,
        "chunk_id": null,
        "wall_time_seconds": 0.1,
        "exit_code": 0,
        "original_token_count": 1,
        "output": "done"
    });
    assert!(serde_json::from_value::<ExecResponse>(value).is_err());
}

#[test]
fn completed_exec_response_omits_daemon_session_id() {
    let response = ExecResponse::Completed(ExecCompletedResponse {
        output: ExecOutputResponse {
            daemon_instance_id: "inst".to_string(),
            running: false,
            chunk_id: None,
            wall_time_seconds: 0.1,
            exit_code: Some(0),
            original_token_count: Some(1),
            output: "done".to_string(),
            warnings: Vec::new(),
        },
    });
    let value = serde_json::to_value(response).unwrap();
    assert!(value.get("daemon_session_id").is_none());
}
```

- [ ] **Step 3: Run failing proto test.**

Run: `cargo test -p remote-exec-proto exec_response`
Expected: compile fails until constructors/callers are updated.

- [ ] **Step 4: Update host constructors and add response accessors.**

Replace `running_response` and `finish_response` bodies to build the enum variants. Add these helper methods on `ExecResponse`:

```rust
impl ExecResponse {
    pub fn running(&self) -> bool {
        self.output().running
    }

    pub fn output(&self) -> &ExecOutputResponse {
        match self {
            Self::Running(response) => &response.output,
            Self::Completed(response) => &response.output,
        }
    }

    pub fn daemon_session_id(&self) -> Option<&str> {
        match self {
            Self::Running(response) => Some(response.daemon_session_id.as_str()),
            Self::Completed(_) => None,
        }
    }
}
```

Use `ExecResponse::Running(ExecRunningResponse { daemon_session_id, output })` for live sessions and `ExecResponse::Completed(ExecCompletedResponse { output })` for completed sessions.

- [ ] **Step 5: Update broker exec handling deliberately.**

In `tools/exec.rs`, stop checking `response.daemon_session_id: Option<_>`. Pattern-match:

```rust
match &response {
    ExecResponse::Running(running) => {
        let daemon_session_id = running.daemon_session_id.clone();
        let output = &running.output;
    }
    ExecResponse::Completed(completed) => {
        let output = &completed.output;
    }
}
```

Keep malformed-response integration tests by making the stub daemon return malformed JSON and asserting deserialization/validation fails with a clear error message.

- [ ] **Step 6: Update daemon and local client method signatures without changing endpoint paths.**

Keep endpoints returning `Json<ExecResponse>`. In `crates/remote-exec-daemon/src/exec/mod.rs`, keep:

```rust
pub async fn exec_start(...) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)>
pub async fn exec_write(...) -> Result<Json<ExecResponse>, (StatusCode, Json<RpcErrorBody>)>
```

In `crates/remote-exec-broker/src/daemon_client.rs` and `crates/remote-exec-broker/src/local_backend.rs`, keep:

```rust
) -> Result<ExecResponse, DaemonClientError>
```

Update every `response.running`, `response.exit_code`, `response.output`, `response.warnings`, and `response.daemon_session_id` field access in broker/host code to use pattern matches or the helper accessors added in Step 4.

- [ ] **Step 7: Run exec tests.**

Run:

```bash
cargo test -p remote-exec-proto exec_response
cargo test -p remote-exec-daemon --test exec_rpc
cargo test -p remote-exec-broker --test mcp_exec
```

Expected: all pass.

- [ ] **Step 8: Commit.**

```bash
git add crates/remote-exec-proto/src/rpc.rs crates/remote-exec-host/src/exec crates/remote-exec-daemon/src/exec crates/remote-exec-broker/src crates/remote-exec-broker/tests crates/remote-exec-daemon/tests
git commit -m "refactor: split exec response session invariants"
```

### Task 9: Introduce Focused Typed IDs

**Finding:** `#21`

**Files:**
- Modify: `crates/remote-exec-host/src/ids.rs`
- Modify: `crates/remote-exec-broker/src/session_store.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-host/src/port_forward/session_store.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host port_forward`
  - `cargo test -p remote-exec-broker port_forward`
  - `cargo test -p remote-exec-broker --test mcp_exec`

**Testing approach:** characterization/integration test
Reason: This is type-safety work over existing behavior. The public wire fields remain strings; internal maps should no longer freely swap ID namespaces.

- [ ] **Step 1: Add ID newtype macro and types.**

In `remote-exec-host/src/ids.rs`, replace the raw string helpers with:

```rust
use std::fmt;

fn uuid_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(prefix: &str) -> Self {
                Self(format!("{prefix}_{}", uuid_suffix()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

id_type!(InstanceId);
id_type!(ExecSessionId);
id_type!(TunnelSessionId);
id_type!(PublicSessionId);
id_type!(ForwardId);

pub fn new_instance_id() -> InstanceId {
    InstanceId::new("inst")
}

pub fn new_exec_session_id() -> ExecSessionId {
    ExecSessionId::new("sess")
}

pub fn new_tunnel_session_id() -> TunnelSessionId {
    TunnelSessionId::new("ptun")
}

pub fn new_public_session_id() -> PublicSessionId {
    PublicSessionId::new("sess")
}

pub fn new_forward_id() -> ForwardId {
    ForwardId::new("fwd")
}
```

- [ ] **Step 2: Convert at wire boundaries first.**

Where RPC structs still require `String`, call `.into_string()` at the boundary. For example, in host state initialization:

```rust
daemon_instance_id: crate::ids::new_instance_id().into_string(),
```

In broker session insertion:

```rust
let session_id = remote_exec_host::ids::new_public_session_id().into_string();
```

- [ ] **Step 3: Keep store fields as strings at the public boundary and tighten generators only.**

Do not change public tool structs, CLI arguments, RPC structs, or serde-visible fields in this task. Convert typed generated IDs to `String` when storing them in existing public-boundary structs:

```rust
let session_id = remote_exec_host::ids::new_public_session_id().into_string();
let forward_id = remote_exec_host::ids::new_forward_id().into_string();
```

This task deliberately prevents accidental namespace swaps at generation sites without broadening into a public API migration.

- [ ] **Step 4: Add compile-time namespace smoke test.**

In `ids.rs`, add:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn generated_ids_keep_expected_prefixes() {
        assert!(super::new_instance_id().as_str().starts_with("inst_"));
        assert!(super::new_exec_session_id().as_str().starts_with("sess_"));
        assert!(super::new_tunnel_session_id().as_str().starts_with("ptun_"));
        assert!(super::new_public_session_id().as_str().starts_with("sess_"));
        assert!(super::new_forward_id().as_str().starts_with("fwd_"));
    }
}
```

- [ ] **Step 5: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-host ids
cargo test -p remote-exec-host port_forward
cargo test -p remote-exec-broker port_forward
cargo test -p remote-exec-broker --test mcp_exec
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src/ids.rs crates/remote-exec-host/src crates/remote-exec-broker/src
git commit -m "refactor: type generated remote exec ids"
```

### Task 10: Replace Anyhow In Proto Port-Forward Helpers

**Finding:** `#22`

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/remote-exec-proto/Cargo.toml`
- Modify: `crates/remote-exec-proto/src/port_forward.rs`
- Test/Verify:
  - `cargo test -p remote-exec-proto port_forward`
  - `cargo test -p remote-exec-broker port_forward`
  - `cargo test -p remote-exec-host port_forward`

**Testing approach:** TDD
Reason: The desired behavior is typed error classification while preserving message text.

- [ ] **Step 1: Add a failing typed-error test.**

In `port_forward.rs` tests, add:

```rust
#[test]
fn invalid_endpoint_returns_typed_error() {
    let err = normalize_endpoint("localhost").unwrap_err();
    assert!(matches!(err, PortForwardProtoError::InvalidEndpoint { .. }));
}

#[test]
fn invalid_port_returns_typed_error() {
    let err = endpoint_port("127.0.0.1:not-a-port").unwrap_err();
    assert!(matches!(err, PortForwardProtoError::InvalidPort { .. }));
}
```

- [ ] **Step 2: Run the failing test.**

Run: `cargo test -p remote-exec-proto port_forward`
Expected: compile fails because `PortForwardProtoError` does not exist.

- [ ] **Step 3: Add `thiserror` to workspace and proto dependencies.**

In root `Cargo.toml`, add this line under `[workspace.dependencies]`:

```toml
thiserror = "2"
```

In `crates/remote-exec-proto/Cargo.toml`, add:

```toml
thiserror = { workspace = true }
```

- [ ] **Step 4: Implement typed error.**

At the top of `port_forward.rs`, replace `use anyhow::{Context, anyhow};` with:

```rust
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PortForwardProtoError {
    #[error("endpoint must not be empty")]
    EmptyEndpoint,
    #[error("invalid endpoint `{endpoint}`; missing `]`")]
    MissingIpv6Bracket { endpoint: String },
    #[error("invalid endpoint `{endpoint}`; expected [host]:port")]
    InvalidIpv6Endpoint { endpoint: String },
    #[error("invalid endpoint `{endpoint}`; expected <port> or <host>:<port>")]
    InvalidEndpoint { endpoint: String },
    #[error("endpoint host must not be empty")]
    EmptyHost,
    #[error("invalid port `{value}`: {message}")]
    InvalidPort { value: String, message: String },
    #[error("connect_endpoint `{endpoint}` must use a nonzero port")]
    ZeroConnectPort { endpoint: String },
}

pub type Result<T> = std::result::Result<T, PortForwardProtoError>;
```

Change public helper signatures from `anyhow::Result<T>` to `Result<T>`, and replace `anyhow::ensure!` / `with_context` / `anyhow!` with explicit `Err(PortForwardProtoError::...)`.

- [ ] **Step 5: Run focused tests.**

Run:

```bash
cargo test -p remote-exec-proto port_forward
cargo test -p remote-exec-broker port_forward
cargo test -p remote-exec-host port_forward
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add Cargo.toml crates/remote-exec-proto/Cargo.toml crates/remote-exec-proto/src/port_forward.rs Cargo.lock crates/remote-exec-broker/src crates/remote-exec-host/src
git commit -m "refactor: type port forward proto errors"
```

### Task 11: Consolidate C++ Path Joining

**Finding:** `#23`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/platform.h`
- Modify: `crates/remote-exec-daemon-cpp/src/platform.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/shell_policy.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/path_utils.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** existing tests + targeted build verification
Reason: This is a C++ helper consolidation. Existing shell/config/server route tests exercise command lookup enough; XP cross-build catches header/platform issues.

- [ ] **Step 1: Make `path_utils::join_path` normalize the child path.**

In `path_utils.cpp`, update `join_path`:

```cpp
std::string join_path(const std::string& base, const std::string& child) {
    std::string normalized_child = child;
#ifdef _WIN32
    std::replace(normalized_child.begin(), normalized_child.end(), '/', '\\');
#endif
    if (base.empty()) {
        return normalized_child;
    }
    std::string joined = base;
#ifdef _WIN32
    std::replace(joined.begin(), joined.end(), '/', '\\');
#endif
    if (joined[joined.size() - 1] != '/' && joined[joined.size() - 1] != '\\') {
        joined.push_back(native_separator());
    }
    joined += normalized_child;
    return joined;
}
```

Add `#include <algorithm>` to `path_utils.cpp`.

- [ ] **Step 2: Update shell policy caller.**

In `shell_policy.cpp`, include `path_utils.h` and change:

```cpp
const std::string candidate = platform::join_path(dir, command);
```

to:

```cpp
const std::string candidate = path_utils::join_path(dir, command);
```

- [ ] **Step 3: Remove platform duplicate.**

Delete `platform::join_path` declaration from `platform.h` and definition from `platform.cpp`.

- [ ] **Step 4: Verify no caller remains.**

Run: `rg -n 'platform::join_path|join_path\\(' crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/include`
Expected: no `platform::join_path`; `path_utils::join_path` and transfer internal `join_path` wrappers are allowed.

- [ ] **Step 5: Run C++ checks.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected: both pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/platform.h crates/remote-exec-daemon-cpp/src/platform.cpp crates/remote-exec-daemon-cpp/src/path_utils.cpp crates/remote-exec-daemon-cpp/src/shell_policy.cpp
git commit -m "refactor: consolidate cpp path joining"
```

### Task 12: Share C++ Directory Creation Helpers

**Finding:** `#24`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/path_utils.h`
- Modify: `crates/remote-exec-daemon-cpp/src/path_utils.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/patch_engine.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-patch`
  - `make -C crates/remote-exec-daemon-cpp test-host-transfer`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** existing tests + targeted verification
Reason: The shared helper must preserve both patch and transfer behavior. Focused C++ tests cover both surfaces.

- [ ] **Step 1: Add shared helpers to path utils.**

In `path_utils.h`, add:

```cpp
void make_directory_if_missing(const std::string& path);
void create_parent_directories(const std::string& path);
```

In `path_utils.cpp`, add includes:

```cpp
#include <cerrno>
#include <stdexcept>
#ifdef _WIN32
#include <direct.h>
#else
#include <sys/stat.h>
#include <sys/types.h>
#endif
```

Add implementations:

```cpp
void make_directory_if_missing(const std::string& path) {
    if (path.empty()) {
        return;
    }
#ifdef _WIN32
    if (_mkdir(path.c_str()) != 0 && errno != EEXIST) {
#else
    if (mkdir(path.c_str(), 0777) != 0 && errno != EEXIST) {
#endif
        throw std::runtime_error("unable to create directory " + path);
    }
}

void create_parent_directories(const std::string& path) {
    const std::string parent = parent_directory(path);
    if (parent.empty()) {
        return;
    }

    std::string current;
    for (std::size_t i = 0; i < parent.size(); ++i) {
        const char ch = parent[i];
        current.push_back(ch);
        if (ch != '/' && ch != '\\') {
            continue;
        }
        if (current.size() == 1) {
            continue;
        }
        if (current.size() == 3 && current[1] == ':') {
            continue;
        }
        current.erase(current.size() - 1);
        make_directory_if_missing(current);
        current.push_back(ch);
    }
    make_directory_if_missing(parent);
}
```

- [ ] **Step 2: Delegate transfer helpers.**

In `transfer_ops_fs.cpp`, change `make_directory_if_missing` to:

```cpp
void make_directory_if_missing(const std::string& path) {
    if (path.empty() || is_directory(path)) {
        return;
    }
    path_utils::make_directory_if_missing(path);
}
```

Change `ensure_parent_directory` create-parent branch to:

```cpp
path_utils::create_parent_directories(path);
```

Keep the `ParentMissing` check before it unchanged.

- [ ] **Step 3: Remove patch duplicate.**

In `patch_engine.cpp`, delete local `make_directory_if_missing` and `create_parent_directories`.

Replace calls:

```cpp
create_parent_directories(path);
```

with:

```cpp
path_utils::create_parent_directories(path);
```

- [ ] **Step 4: Verify duplicate functions are gone.**

Run: `rg -n 'void make_directory_if_missing|void create_parent_directories' crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/include`
Expected: definitions only in `path_utils.cpp` plus the transfer namespace wrapper declaration/definition in `transfer_ops_internal.h` and `transfer_ops_fs.cpp`.

- [ ] **Step 5: Run focused C++ tests.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-patch
make -C crates/remote-exec-daemon-cpp test-host-transfer
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/path_utils.h crates/remote-exec-daemon-cpp/src/path_utils.cpp crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp crates/remote-exec-daemon-cpp/src/patch_engine.cpp
git commit -m "refactor: share cpp directory creation helpers"
```

### Task 13: Remove Redundant C++ Port-Forward Worker Field

**Finding:** `#25`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_streaming.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_runtime.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-config`
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** existing tests + targeted build verification
Reason: This removes a redundant config field while preserving the existing config key and nested value.

- [ ] **Step 1: Remove the struct member.**

In `include/config.h`, delete:

```cpp
unsigned long port_forward_max_worker_threads;
```

Keep `PortForwardLimitConfig port_forward_limits;`.

- [ ] **Step 2: Remove assignment in `load_config`.**

In `config.cpp`, delete:

```cpp
config.port_forward_max_worker_threads = config.port_forward_limits.max_worker_threads;
```

Do not remove parsing of the config key `port_forward_max_worker_threads`; it still populates `config.port_forward_limits.max_worker_threads`.

- [ ] **Step 3: Update tests and fixtures.**

Replace assertions:

```cpp
assert(config.port_forward_max_worker_threads == 17UL);
assert(sandbox_config.port_forward_max_worker_threads == DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
```

with:

```cpp
assert(config.port_forward_limits.max_worker_threads == 17UL);
assert(sandbox_config.port_forward_limits.max_worker_threads == DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS);
```

In test fixture setup, delete assignments like:

```cpp
config.port_forward_max_worker_threads = DEFAULT_PORT_FORWARD_MAX_WORKER_THREADS;
state.config.port_forward_max_worker_threads = limits.max_worker_threads;
```

and rely on `config.port_forward_limits.max_worker_threads`.

- [ ] **Step 4: Verify no field use remains.**

Run: `rg -n 'port_forward_max_worker_threads' crates/remote-exec-daemon-cpp`
Expected: occurrences remain only in config key parsing/validation text, docs/comments, and tests that write the config key. There must be no `config.port_forward_max_worker_threads` member access.

- [ ] **Step 5: Run C++ checks.**

Run:

```bash
make -C crates/remote-exec-daemon-cpp test-host-config
make -C crates/remote-exec-daemon-cpp check-posix
make -C crates/remote-exec-daemon-cpp check-windows-xp
```

Expected: all pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/config.h crates/remote-exec-daemon-cpp/src/config.cpp crates/remote-exec-daemon-cpp/tests
git commit -m "refactor: remove redundant cpp port forward worker field"
```

### Task 14: Phase B Integration Verification

**Files:**
- No planned source edits. Verification fixes go in a separate follow-up commit from this task.
- Test/Verify:
  - `cargo fmt --all --check`
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** full quality gate
Reason: Phase B touches shared protocol types and both Rust/C++ paths. Focused tests are not enough at the end.

- [ ] **Step 1: Run Rust formatting check.**

Run: `cargo fmt --all --check`
Expected: no diff required.

- [ ] **Step 2: Run workspace tests.**

Run: `cargo test --workspace`
Expected: all tests pass.

- [ ] **Step 3: Run workspace clippy.**

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Run C++ POSIX check.**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: all host tests and POSIX daemon build pass.

- [ ] **Step 5: Run C++ XP cross-build.**

Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
Expected: XP daemon and configured Wine tests/build targets pass, or the command clearly reports that Wine-only runtime tests are skipped by the Makefile.

- [ ] **Step 6: Commit any verification-only fixes.**

If any command required a code fix, commit that fix separately:

```bash
git add <changed-files>
git commit -m "fix: resolve phase b integration fallout"
```

If no files changed, do not create an empty commit.
