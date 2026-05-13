# Round 5 Phase 2 Verified Split God Modules Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Round 5 Phase 2 god-module findings with mechanical internal Rust refactors, while excluding stale claims that no longer apply to the current codebase.

**Requirements:**
- Cover the verified current-code follow-up for audit items `#8`, `#9`, `#10`, `#12`, `#13`, and `#14`.
- Exclude audit item `#11` from execution because the duplicated dual-ownership TCP read-loop seam is already collapsed in current code.
- Preserve public MCP behavior, PKI output paths and permissions, transfer archive semantics, and broker/daemon port-forward recovery behavior.
- Keep the work mechanical and internal: file splits, re-exports, and narrow helper extraction only. Do not widen this phase into the broader ownership and boundary redesigns reserved for later Round 5 phases.
- Continue the user's plan-based execution style and commit after each real task with no empty commits.

**Architecture:** Execute this phase in three medium-sized Rust batches plus a final sweep. First, reduce the broker port-forward god modules by splitting `supervisor.rs` into focused internal modules and collapsing the remaining repeated TCP send/classify branch shape in `tcp_bridge.rs`. Second, move large test blocks out of production modules in `remote-exec-host` and split `remote-exec-pki::write` into focused internal modules while preserving the existing public `remote_exec_pki::write::*` surface. Third, split the host transfer archive export implementation into focused submodules for path preparation, single-source export, and bundle assembly without changing wire-visible archive contents or warnings.

**Verification Strategy:** Verify each batch with the narrowest existing Rust tests that cover the touched seam, then finish with the full Rust quality gate required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features -- -D warnings`. Because this phase is scoped to Rust-only internal refactors, no C++ verification is required unless execution proves a shared contract change reached a C++ path.

**Assumptions / Open Questions:**
- Audit item `#8` is only partially current: Phase 1 already centralized part of the broker tunnel recovery flow, but `tcp_bridge.rs` still contains repeated send/backpressure/retryable/fatal handling that should collapse behind one narrow helper.
- Audit item `#11` is stale in the current repo: `crates/remote-exec-host/src/port_forward/tcp.rs` now routes both ownership modes through `TcpReadLoopTarget` and one shared `tunnel_tcp_read_loop(...)`.
- The safest shape for items `#10` and `#13` is likely `mod tests;` with sibling `tests.rs` files rather than inventing broader new helper layers unless the existing duplication clearly warrants a tiny `test_support` module.
- The safest shape for items `#12` and `#14` is likely converting single files into directory modules (`mod.rs` plus focused siblings) while preserving the current public imports and function names.

**Planning-Time Verification Summary:**
- `#8`: still live, but narrower than the audit text after Phase 1 dedup.
- `#9`: still live as written; `supervisor.rs` remains a large multi-concern owner.
- `#10`: still live as written; `host/port_forward/mod.rs` still mixes production code and a very large test block.
- `#11`: stale; no execution task should be created for it.
- `#12`: still live as written; bundle orchestration, Unix writing, and Windows ACL code remain in one file.
- `#13`: still live as written; `exec/store.rs` still embeds all tests inline.
- `#14`: still live as written; export preparation, single-source export, and bundle assembly still share one file.

---

### Task 1: Broker Port-Forward Module Decomposition

**Intent:** Reduce the largest remaining broker-side port-forward god modules without changing runtime behavior or widening into later ownership redesigns.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely delete/replace: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/supervisor/mod.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/mod.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/events.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`

**Notes / constraints:**
- Preserve the existing internal import shape for `ForwardRuntime`, `ListenSessionControl`, and the reconnect/open helpers as much as practical so the split stays mechanical.
- Keep the already-added Phase 1 helpers (`recoverable_tunnel_frame(...)`, `handle_forward_loop_control(...)`) as the current owner boundaries rather than inventing a larger framework.
- Collapse only the remaining repeated TCP send/backpressure/retryable/fatal logic; do not force UDP into the same abstraction unless execution shows a trivial reuse path.
- Preserve branch-specific behavior such as `close_active_tcp_listen_streams(...)`, dropped-stream accounting, and close-pair semantics.

**Verification:**
- Run: `cargo test -p remote-exec-broker --lib port_forward::`
- Expect: broker port-forward unit coverage still passes after the internal split.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: Rust-daemon public forwarding behavior still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker forwarding behavior against the C++ daemon still passes unchanged.

- [ ] Confirm the current top-level concern boundaries inside `supervisor.rs` and map them into `mod.rs`, `open.rs`, and `reconnect.rs`
- [ ] Extract one narrow helper for the remaining repeated TCP send/classify control flow without changing close/recover side effects
- [ ] Convert `supervisor.rs` into focused internal submodules and update internal imports with minimal surface churn
- [ ] Run focused broker forwarding verification across unit and public integration coverage
- [ ] Commit with real changes only

### Task 2: Host Test Relocation And PKI Write Module Split

**Intent:** Remove large test blocks from production modules and split PKI write responsibilities into focused internal modules while preserving the current public API.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely create: `crates/remote-exec-host/src/port_forward/tests.rs`
- Likely create: `crates/remote-exec-host/src/port_forward/test_support.rs`
- Likely modify: `crates/remote-exec-host/src/exec/store.rs`
- Likely create: `crates/remote-exec-host/src/exec/store/tests.rs`
- Likely delete/replace: `crates/remote-exec-pki/src/write.rs`
- Likely create: `crates/remote-exec-pki/src/write/mod.rs`
- Likely create: `crates/remote-exec-pki/src/write/bundle.rs`
- Likely create: `crates/remote-exec-pki/src/write/unix.rs`
- Likely create: `crates/remote-exec-pki/src/write/windows_acl.rs`
- Likely create: `crates/remote-exec-pki/src/write/tests.rs`
- Existing references: `crates/remote-exec-pki/src/lib.rs`
- Existing references: `crates/remote-exec-admin/src/`

**Notes / constraints:**
- Keep the production/test split mechanical for `port_forward` and `exec/store`; do not widen this into behavioral test rewrites.
- Preserve the current test helper semantics for TCP echo/hold/non-draining servers if they move into `test_support.rs`.
- Preserve the current public `remote_exec_pki::write::*` import surface and all existing output-path, overwrite, and permission behavior.
- Keep the Windows private-key ACL implementation isolated rather than partially duplicated between modules.

**Verification:**
- Run: `cargo test -p remote-exec-host port_forward::port_tunnel_tests`
- Expect: moved host port-forward tests still pass through the new test module layout.
- Run: `cargo test -p remote-exec-host exec::store::tests`
- Expect: exec store coverage still passes after the test move.
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI crate tests still pass with the `write` module split.
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Expect: admin workflows using the PKI write surface still pass unchanged.

- [ ] Move the host port-forward integration-style tests into dedicated test files and extract only the helper support that clearly reduces duplication
- [ ] Move `exec/store.rs` tests into a sibling test module without changing store behavior
- [ ] Convert `remote-exec-pki::write` into a directory module with focused bundle, Unix, and Windows ACL ownership while preserving the exported API
- [ ] Run focused host, PKI, and admin verification for the moved code
- [ ] Commit with real changes only

### Task 3: Host Transfer Archive Export Split

**Intent:** Split the host archive export implementation into smaller internal modules while preserving the same archive contents, warnings, and transfer-facing behavior.

**Relevant files/components:**
- Likely delete/replace: `crates/remote-exec-host/src/transfer/archive/export.rs`
- Likely create: `crates/remote-exec-host/src/transfer/archive/export/mod.rs`
- Likely create: `crates/remote-exec-host/src/transfer/archive/export/prepare.rs`
- Likely create: `crates/remote-exec-host/src/transfer/archive/export/single.rs`
- Likely create: `crates/remote-exec-host/src/transfer/archive/export/bundle.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/mod.rs`
- Existing references: `crates/remote-exec-daemon/src/transfer/`
- Existing references: `crates/remote-exec-broker/src/tools/transfer/`
- Existing references: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_transfer.rs`

**Notes / constraints:**
- Preserve the current exported function names and call sites where practical; this should be an internal ownership split, not a transfer API redesign.
- Keep file/symlink/directory export semantics unchanged, including `SINGLE_FILE_ENTRY`, warning generation, and transfer summary archive entries.
- Avoid widening this task into import-path cleanup unless the module split requires a small mechanical adjustment.
- If execution shows that a different file naming scheme is clearer than `prepare/single/bundle`, keep the same responsibility boundaries and document the chosen shape in the implementation notes.

**Verification:**
- Run: `cargo test -p remote-exec-host --lib transfer::`
- Expect: host transfer/archive unit coverage still passes after the export split.
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: daemon transfer RPC coverage still passes against the split host export implementation.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker public transfer behavior still passes end-to-end.

- [ ] Confirm the current export responsibilities and map them into focused internal submodules for preparation, single-source export, and multi-source bundling
- [ ] Convert `export.rs` into a directory module while preserving the current outward-facing function names
- [ ] Keep archive structure, error mapping, and warning/summary behavior stable across the split
- [ ] Run focused host, daemon, and broker transfer verification
- [ ] Commit with real changes only

### Task 4: Final Phase 2 Confirmatory Sweep

**Intent:** Reconfirm that the verified Round 5 Phase 2 seams were actually reduced as intended and that the cross-cutting Rust refactor remains clean under the repo quality gate.

**Relevant files/components:**
- Likely inspect: `docs/CODE_AUDIT_ROUND5.md`
- Likely inspect: the files touched by Tasks 1 through 3

**Notes / constraints:**
- Re-run targeted searches so the final notes distinguish stale claims from actually remediated ones.
- Keep the sweep scoped to verified Phase 2 items only; do not roll directly into Phase 3 ownership work from here.
- If any remaining code shape survives intentionally because a narrower mechanical boundary proved safer, document that instead of forcing a riskier abstraction.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the Rust workspace passes end-to-end after the full Phase 2 bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting stays clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions after the module splits.

- [ ] Re-run targeted searches for the verified Phase 2 seams and confirm which audit claims were fully live, partially stale, or stale
- [ ] Run the full Rust quality gate required for these cross-cutting internal refactors
- [ ] Summarize item `#11` as stale and item `#8` as partially reduced before execution started
- [ ] Commit any final sweep-only adjustments if needed, otherwise do not create an empty commit
