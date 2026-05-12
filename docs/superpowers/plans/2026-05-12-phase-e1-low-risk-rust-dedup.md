# Phase E1 Low-Risk Rust Dedup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Land the next low-risk Phase E1 Rust maintainability slice by consolidating small duplicated helpers in broker, host, and proto code without changing public behavior.

**Requirements:**
- Cover the approved low-risk Rust bundle: audit items `#6`, `#7`, `#8`, `#10`, and `#11`.
- Keep broker tool behavior, host transfer behavior, and public schemas unchanged.
- Avoid widening this batch into forwarding structural work, C++ cleanup, PKI/logging dedup, or contract changes.
- Continue the user’s plan-based execution style.
- Commit after each task only when that task has real content changes; do not create empty commits.

**Architecture:** Deduplicate each concern at the narrowest owner boundary instead of introducing a broad new abstraction layer. `remote-exec-host::transfer::archive` should own one internal transfer-error conversion helper reused by import/export. `remote-exec-broker::daemon_client` should expose one RPC-message-to-`anyhow` conversion helper reused by image and transfer call sites. `remote-exec-proto::path` should own the current-host path-policy helper used by broker runtime code. Broker request-context target aggregation and exec response validation should each collapse to one helper local to their existing modules.

**Verification Strategy:** Use focused broker integration tests that exercise the touched seams: `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test -p remote-exec-broker --test mcp_assets`, `cargo test -p remote-exec-broker --test mcp_exec`, and `cargo test -p remote-exec-broker --test mcp_forward_ports` as appropriate per task. Prefer task-local verification after each code batch instead of one large end-only test sweep.

**Assumptions / Open Questions:**
- `mcp_transfer` is the primary regression guard for host archive helper changes because the host archive helpers are reached through broker transfer flows.
- `mcp_assets` is the most direct guard for `view_image` error normalization because it already covers broker asset/tool behavior.
- `host_policy()` should live in `remote_exec_proto::path` unless execution reveals an existing better home in the proto layer; do not create a new crate or utility module for this batch.
- If a proposed dedup helper would force awkward cross-module visibility or behavioral drift, keep the helper local and narrow rather than generalizing further.

---

### Task 1: Save The Phase E1 Low-Risk Rust Dedup Plan

**Intent:** Create the tracked plan artifact for the approved second E1 slice before implementation starts.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-12-phase-e1-low-risk-rust-dedup.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start code edits for this slice until the plan is reviewed and approved.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-12-phase-e1-low-risk-rust-dedup.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged design + execution plan at the tracked path
- [ ] Check the header, goal, and scope against the approved bundle
- [ ] Confirm the plan stays limited to the selected Rust dedup items
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: Deduplicate Transfer Archive Errors And Broker RPC Message Normalization

**Intent:** Remove the small duplicated error-conversion helpers that currently live in multiple host/broker files while preserving existing error text behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/transfer/archive/mod.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/export.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/import.rs`
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/image.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/operations.rs`

**Notes / constraints:**
- Keep the host archive helper internal to the archive module; do not broaden visibility beyond what import/export actually need.
- Keep broker daemon RPC normalization focused on the existing `Rpc { message, .. }` behavior; do not alter transport/decode wrapping semantics.
- Avoid touching unrelated transfer behavior or changing user-visible message wording unless the current wording is already derived from the shared branch.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: transfer behavior and archive-backed flows still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: asset/view-image behavior still passes after image error-normalization dedup.

- [ ] Confirm the duplicated helper bodies still match the audit and current tree
- [ ] Add one shared archive transfer-error helper and update import/export to use it
- [ ] Add one shared broker daemon RPC normalization helper and update image/transfer call sites
- [ ] Run focused verification for transfer and asset flows
- [ ] Commit with real code changes only

### Task 3: Deduplicate Host Path Policy, Exec Response Validation, And Multi-Target Request Context

**Intent:** Consolidate the remaining obvious Rust helper duplication in proto and broker runtime code without altering tool behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/path.rs`
- Likely modify: `crates/remote-exec-broker/src/startup.rs`
- Likely modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Likely modify: `crates/remote-exec-broker/src/request_context.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/port_forward.rs`

**Notes / constraints:**
- `host_policy()` belongs in proto because both policy choice and policy users are already proto-centric; keep it simple and do not build a new utility crate.
- Collapse the duplicated exec response validators into one helper without changing the existing running/completed output invariants.
- Collapse transfer/forward multi-target context collection into one request-context helper that still sorts, deduplicates, filters empties, and joins targets the same way as today.
- Do not fold unrelated cleanups like transfer options dedup (`#41`) into this batch.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: exec flows, including malformed-response handling, still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: transfer flows still pass with the shared host-policy and request-context helper.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: forward-port tool behavior still passes with the shared multi-target context helper.

- [ ] Add proto-owned `host_policy()` and switch the scoped broker call sites to use it
- [ ] Collapse the duplicated exec response validation into one helper
- [ ] Add one request-context multi-target helper and switch transfer/forward tools to it
- [ ] Run focused verification for exec, transfer, and forwarding flows
- [ ] Commit with real code changes only
