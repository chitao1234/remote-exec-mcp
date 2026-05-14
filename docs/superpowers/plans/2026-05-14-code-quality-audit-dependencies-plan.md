# Code Quality Audit Dependencies Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live dependency-management issues from section `7.*` of `docs/code-quality-audit.md` with a small, low-risk manifest-focused pass.

**Requirements:**
- Fix only the verified live `7.*` items; do not force changes for stale claims.
- Preserve runtime behavior, public wire contracts, and current broker/daemon/C++ feature scope.
- Keep `x509-parser` as a direct dependency of `remote-exec-pki` because the crate calls it directly; the cleanup is version-centralization, not dependency removal.
- Treat the current `x509-parser` situation as a future divergence risk, not a live duplicate-version bug.
- Keep `tempfile` in `remote-exec-broker` runtime dependencies unless production transfer code is redesigned; `7.4` is explicitly out of scope for implementation.
- Scope any `tokio` feature narrowing to changes that are directly supported by current code usage and fresh verification.
- Do not edit `docs/code-quality-audit.md`; it remains historical input, not the live contract.

**Architecture:** Treat this as two small implementation tasks plus a final sweep. First, centralize `x509-parser` version management in the workspace while keeping `remote-exec-pki`'s direct dependency intact. Second, narrow the broad workspace `tokio` feature surface only where the current crate usage proves it is safe and worthwhile, with `remote-exec-proto` as the primary pressure point rather than a broad multi-crate redesign.

**Verification Strategy:** Use focused Rust verification after each task, then finish with a small cross-workspace compile/test sweep to confirm the manifest changes did not break downstream crates.

**Assumptions / Open Questions:**
- `7.3` is only partially a dependency-management cleanup. Because Cargo feature unification can reduce the benefit of workspace-level changes, execution should prefer the smallest manifest reshaping that still improves the declared dependency surface.
- `remote-exec-pki` does not use `tokio`, so the audit's mention of it under `7.3` is stale and should not drive any work.
- If narrowing workspace-level `tokio` features proves too coupled to current crate inheritance, execution may need to move one or more crates off the shared workspace `tokio` declaration rather than trying to solve everything from the root manifest.

**Planning-Time Verification Summary:**
- `7.1`: valid. `crates/remote-exec-pki/Cargo.toml` still declares `x509-parser = "0.18"` directly instead of using workspace dependency management.
- `7.2`: partially valid and narrowed. `remote-exec-pki` currently depends on `x509-parser` both directly and transitively through `rcgen`, but the resolved tree currently lands on one version, `x509-parser v0.18.1`, so there is no live duplicate-version problem. The real issue is future version drift risk.
- `7.3`: partially valid and strategic. The workspace `tokio` feature list in `Cargo.toml` is broad, and `crates/remote-exec-proto/Cargo.toml` inherits it despite using a narrower slice of `tokio`.
- `7.4`: invalid/stale. `crates/remote-exec-broker/Cargo.toml` uses `tempfile` in production transfer code, including `crates/remote-exec-broker/src/tools/transfer/operations.rs`.

---

### Task 1: Centralize `x509-parser` Version Management

**Intent:** Move `x509-parser` version ownership into the workspace while preserving the direct `remote-exec-pki` dependency needed by production code.

**Relevant files/components:**
- Likely modify: `Cargo.toml`
- Likely modify: `crates/remote-exec-pki/Cargo.toml`
- Existing references: `crates/remote-exec-pki/src/generate.rs`

**Notes / constraints:**
- Cover verified audit items `7.1` and the real, narrowed portion of `7.2`.
- Do not remove the direct `x509-parser` dependency from `remote-exec-pki`; `generate.rs` uses the crate API directly.
- Keep `rcgen` feature usage unchanged unless execution confirms there is a safe simplification.

**Verification:**
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI tests still pass after the manifest cleanup.
- Run: `cargo tree -p remote-exec-pki | rg "x509-parser|rcgen"`
- Expect: the crate still resolves cleanly, with version management now sourced from workspace dependencies.

- [ ] Confirm the current direct and transitive `x509-parser` usage in `remote-exec-pki`
- [ ] Add `x509-parser` to workspace dependency management and switch `remote-exec-pki` to `workspace = true`
- [ ] Verify that the resolved dependency tree still matches the expected single-version state
- [ ] Run focused PKI verification
- [ ] Commit

### Task 2: Narrow The `tokio` Dependency Surface Where It Is Actually Over-Broad

**Intent:** Reduce the broad `tokio` feature declaration only where current crate usage proves that the smaller surface is real and maintainable.

**Relevant files/components:**
- Likely modify: `Cargo.toml`
- Likely modify: `crates/remote-exec-proto/Cargo.toml`
- Likely inspect: `crates/remote-exec-proto/src/`
- Likely inspect: `crates/remote-exec-host/Cargo.toml`
- Likely inspect: `crates/remote-exec-daemon/Cargo.toml`
- Likely inspect: `crates/remote-exec-broker/Cargo.toml`

**Notes / constraints:**
- Cover the verified, narrowed portion of `7.3`.
- Start from actual `tokio` usage, not hypothetical minimalism.
- Prefer the smallest manifest change that meaningfully improves declared dependency scope.
- If workspace-level narrowing is too coupled, it is acceptable to make `remote-exec-proto` stop inheriting the full workspace `tokio` declaration and give it an explicit smaller feature set instead.
- Do not spend time trying to eliminate features that are clearly required by `remote-exec-host`, `remote-exec-daemon`, or `remote-exec-broker`.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Expect: proto codec and port-tunnel tests still pass.
- Run: `cargo check --workspace`
- Expect: downstream crates still compile after the `tokio` manifest changes.

- [ ] Confirm the actual `tokio` surface used by `remote-exec-proto` and the runtime crates
- [ ] Apply the smallest safe manifest change that narrows the over-broad `tokio` declaration
- [ ] Re-check that the narrowed declaration still supports the current test and compile paths
- [ ] Run focused proto verification plus a workspace compile check
- [ ] Commit

### Task 3: Final `7.*` Sweep And Explicit Non-Actionable Confirmation

**Intent:** Confirm that the verified dependency issues were fixed or intentionally excluded, and that stale claims were not accidentally implemented.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: `Cargo.toml`
- Likely inspect: `crates/remote-exec-pki/Cargo.toml`
- Likely inspect: `crates/remote-exec-broker/Cargo.toml`

**Notes / constraints:**
- Keep this sweep limited to section `7.*`.
- Reconfirm that `7.4` remains intentionally untouched because the broker uses `tempfile` in production transfer logic.
- If Task 2 only partially narrows `tokio`, record that as an intentional boundary rather than forcing a wider manifest redesign.

**Verification:**
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI behavior remains green after the manifest updates.
- Run: `cargo test -p remote-exec-proto`
- Expect: proto tests remain green.
- Run: `cargo check --workspace`
- Expect: the workspace still compiles after the dependency cleanup.

- [ ] Re-run the `7.*` verification queries and confirm the final shape is intentional
- [ ] Re-run the focused PKI and proto tests
- [ ] Re-run the workspace compile check
- [ ] Summarize which `7.*` items were fixed, narrowed, or intentionally excluded
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
