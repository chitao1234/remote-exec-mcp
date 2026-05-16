# Section 6 Local Duplication Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Remove the verified section 6 local duplication and wrapper churn without adding unnecessary abstraction layers, crate reshuffles, or public behavior changes.

**Requirements:**
- Preserve the public MCP surface, broker-owned ID namespaces, current local-target behavior, and v4 port-forward wire format.
- Fix the verified section 6 cleanup items: `6.1`, `6.2`, `6.3`, `6.5`, `6.6`, and `6.7`.
- Treat `6.4` as a boundary cleanup inside broker enum dispatch, not as a mandate to introduce a new trait-object abstraction.
- Do not remove `remote-exec-util` in this pass. `6.8` is not accepted as a standalone crate-deletion task because shared logging initialization still has a legitimate common home.
- Keep malformed or intentionally partial tunnel-meta fixtures hand-built where tests are explicitly validating fallback behavior rather than valid schema encoding.
- Avoid README, config-example, or skill changes unless implementation discovers a real observable behavior change. This plan is for internal cleanup.

**Architecture:** This pass should tighten existing local seams rather than invent a new abstraction stack. On the host side, use the canonical proto path-policy helpers directly and remove local wrappers that only restate the same contract with different names or parameter order. On the broker side, make the local port-forward runtime policy explicit at the point where it is constructed, move the shared remote/local operation subset onto the existing `TargetBackend` enum instead of a macro, and fold duplicated RPC decode helpers into a single policy-driven helper. For logging/test cleanup, remove unnecessary `preview_text` wrapper hops and build valid tunnel test frames through the existing typed encoders so schema drift fails loudly.

**Verification Strategy:** Run focused Rust tests after each task, then finish the implementation pass with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`. The main regression surfaces here are host path/sandbox handling, broker local-target execution and assets, and broker port-forward tests that exercise tunnel framing.

**Assumptions / Open Questions:**
- `LocalTargetConfig::embedded_port_forward_host_config` currently has a single broker-local caller. Implementation should confirm that and then move or replace the helper so the port-forward-specific policy lives next to `local_port_backend`.
- The `dispatch_backend!` macro exists only because the shared remote/local broker operation subset is not owned by a normal Rust type. This should be solved by `TargetBackend` methods, not by `dyn` dispatch or boxed async trait plumbing.
- `preview_text` wrapper cleanup should not pull `tracing_subscriber` concerns into `remote-exec-proto`; the accepted scope is to reduce wrapper layers, not to flatten workspace dependencies indiscriminately.

---

### Task 1: Deduplicate host path-policy and sandbox wrapper seams

**Intent:** Remove local helpers that restate shared path-policy behavior or invert canonical argument order.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/host_path.rs`
- Likely modify: `crates/remote-exec-host/src/sandbox.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/mod.rs`
- Existing references: `crates/remote-exec-proto/src/path.rs`
- Existing references: `crates/remote-exec-host/src/path_compare.rs`

**Notes / constraints:**
- Prefer the canonical `remote_exec_proto::path::host_policy()` logic over duplicate host-local implementations.
- If preserving the `host_path` module API is useful, use a re-export or single-source alias rather than another hand-written wrapper body.
- Standardize sandbox callers on the canonical `path_compare::path_is_within(path, root)` argument order.
- Keep Windows `windows_posix_root` and host path normalization behavior unchanged.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: path normalization, sandbox authorization, and transfer path handling remain behaviorally unchanged.

- [ ] Inspect the existing host path-policy and sandbox wrapper call sites and confirm the exact local helper set to remove
- [ ] Add or update tests only if the cleanup changes the seam enough to deserve direct coverage
- [ ] Replace duplicate host path-policy wrappers with canonical shared helpers or re-exports
- [ ] Delete the sandbox-local `path_is_within` argument-flipping wrapper and update call sites
- [ ] Run focused host and broker verification
- [ ] Commit

### Task 2: Make broker-local runtime policy and backend dispatch explicit

**Intent:** Clean up the broker-local duplication while keeping the current concrete backend types and public behavior intact.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely modify: `crates/remote-exec-broker/src/local_port_backend.rs`
- Likely modify: `crates/remote-exec-broker/src/target/backend.rs`
- Likely modify: `crates/remote-exec-broker/src/target/handle.rs`
- Likely modify: `crates/remote-exec-broker/src/local_backend.rs`
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`

**Notes / constraints:**
- Move the broker-local port-forward runtime defaults out of the generic config factory seam and next to the local port-forward runtime construction, or otherwise make that policy ownership explicit at the port-forward boundary.
- Replace `dispatch_backend!` with normal `TargetBackend` methods for the shared remote/local operation subset (`target_info`, `exec_start`, `exec_write`, `patch_apply`, `image_read`).
- Keep `DaemonClient` and `LocalDaemonClient` as separate concrete types; do not introduce `dyn` traits, `async-trait`, or boxed-future plumbing just to satisfy the audit literally.
- Fold strict/lenient RPC error decoding into one helper keyed by the existing read-policy distinction, so the duplication disappears without changing error semantics.
- Preserve `RemoteTargetHandle` for genuinely remote-only operations such as transfer streaming and upgraded port tunnels.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: local exec/assets behavior stays stable, local port-forward bootstrap still works, and daemon-client RPC error handling remains unchanged.

- [ ] Confirm the single-caller shape of the broker-local port-forward embedded host config helper
- [ ] Add or update focused tests if the config or dispatch seam becomes materially clearer to cover directly
- [ ] Move or reshape broker-local port-forward runtime defaults so the policy decision lives with the local port-forward runtime path
- [ ] Replace the backend dispatch macro with enum-owned methods on `TargetBackend`
- [ ] Collapse strict and lenient RPC error decoding into one policy-driven helper
- [ ] Run focused broker verification
- [ ] Commit

### Task 3: Remove extra preview-text hops and type valid tunnel test fixtures

**Intent:** Reduce local wrapper churn and make valid tunnel test frames use the same typed encoding path as production code.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/lib.rs`
- Likely modify: `crates/remote-exec-host/src/logging.rs`
- Likely modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Likely modify: `crates/remote-exec-broker/src/logging.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/patch.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Existing references: `crates/remote-exec-util/src/lib.rs`

**Notes / constraints:**
- Keep `remote-exec-util` as the shared home for logging initialization and the underlying `preview_text` helper in this pass.
- Remove host/broker wrapper layers only where they are pure pass-throughs; do not entangle this with a broader dependency-topology redesign.
- Convert valid tunnel test frames to typed meta structs plus `encode_tunnel_meta(...)` where the schema already exists.
- Leave malformed JSON or intentionally partial metadata fixtures hand-authored when the test is specifically about fallback decode behavior.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Run: `cargo fmt --all --check`
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: helper cleanup is behavior-neutral, tunnel tests still cover valid and fallback frame paths correctly, and the Rust workspace passes formatting and lint gates.

- [ ] Inspect the current host and broker `preview_text` call sites and remove pure pass-through wrappers where direct shared-helper use is clearer
- [ ] Update valid tunnel test fixtures to use typed meta encoding and retain manual malformed fixtures only where fallback behavior is under test
- [ ] Keep the shared logging-init helper in `remote-exec-util` and avoid a workspace-crate removal in this task
- [ ] Run focused port-forward verification and final Rust quality gates
- [ ] Commit
