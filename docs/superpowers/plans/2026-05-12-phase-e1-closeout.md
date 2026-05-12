# Phase E1 Close-Out Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Finish Phase E1 by landing the two remaining dedup findings and then running a bounded confirmatory sweep across the full E1 surface.

**Requirements:**
- Cover the remaining E1 findings: `#20` and residual `#21` from `docs/CODE_AUDIT_ROUND4.md`.
- Include a final confirmatory sweep for the earlier E1 findings `#6` through `#19` so the phase ends with fresh verification evidence rather than assumption.
- Keep public MCP behavior, PKI output filenames, logging defaults, and earlier E1 forwarding/tool behavior unchanged.
- Keep the logging extraction narrow; do not create a general-purpose dumping-ground utility layer.
- Continue the user’s plan-based execution style and commit after each task only when that task has real code changes; do not create empty commits.

**Architecture:** Keep `#20` fully local to `remote-exec-pki::write` by centralizing the repeated `KeyPairPaths` construction and pair writing behind one narrow helper without changing manifest contents or output layout. For residual `#21`, add a tiny `remote-exec-util` workspace crate that owns only the duplicated Rust logging/text helpers needed here: `preview_text(raw, limit)` and a parameterized `init_logging(default_filter)`. Broker and daemon keep thin local wrappers for their crate-specific default filters, while host consumes the shared text helper without turning the new crate into a broader cross-cutting abstraction.

**Verification Strategy:** Verify each code-bearing task with the narrowest existing regression coverage first, then finish with a bounded E1 confirmatory sweep. PKI changes should be checked through `cargo test -p remote-exec-pki --test dev_init_bundle` and widened to `cargo test -p remote-exec-admin --test dev_init` if the touched seam reaches the admin CLI path. Logging/helper extraction should be checked with focused broker and daemon tests that compile and exercise the touched crates. The final sweep should rerun the focused E1 commands already used in earlier batches: `cargo test -p remote-exec-broker --test mcp_transfer`, `cargo test -p remote-exec-broker --test mcp_assets`, `cargo test -p remote-exec-broker --test mcp_exec`, `cargo test -p remote-exec-broker --test mcp_forward_ports`, `cargo test -p remote-exec-daemon --test port_forward_rpc`, and `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`, alongside targeted `rg` scans keyed to findings `#6` through `#21`.

**Assumptions / Open Questions:**
- `remote-exec-util` should stay minimal and workspace-local; if execution reveals a better existing home for `preview_text` or `init_logging`, reuse it only if that keeps the scope as narrow as the new crate.
- The PKI helper for `#20` should centralize path construction and pair writing, but the public helper names (`write_ca_pair`, `write_broker_pair`, `write_daemon_pair`) may remain as thin wrappers if that preserves the current call surface cleanly.
- The confirmatory sweep is meant to confirm or refute residual E1 issues, not to reopen later-phase cleanup items such as `#41` or E2/E3 restructuring.
- If the sweep finds a true remaining E1 issue outside `#20` and `#21`, fix it only if it is small, clearly within E1, and can be landed without re-planning the whole phase.

---

### Task 1: Save The Phase E1 Close-Out Plan

**Intent:** Create the tracked plan artifact for the final E1 close-out slice before implementation starts.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-12-phase-e1-closeout.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start code edits for the final E1 slice until the plan is reviewed and approved.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-12-phase-e1-closeout.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged design + execution plan at the tracked path
- [ ] Check the header, goal, and scope against the approved close-out design
- [ ] Confirm the plan stays limited to `#20`, residual `#21`, and the bounded E1 confirmatory sweep
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: Deduplicate PKI Pair Path Construction And Writes

**Intent:** Remove the repeated certificate/key pair path construction in `remote-exec-pki::write` while preserving the existing dev-init bundle layout and public helper behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-pki/src/write.rs`
- Existing references: `crates/remote-exec-pki/src/manifest.rs`
- Existing references: `crates/remote-exec-pki/src/lib.rs`
- Existing references: `crates/remote-exec-admin/src/` if an integration call site needs verification context only

**Notes / constraints:**
- Preserve filenames exactly: `ca.pem`, `ca.key`, `broker.pem`, `broker.key`, and `{target}.pem` / `{target}.key`.
- Keep `write_dev_init_bundle` behavior unchanged apart from reusing the centralized path-construction helper.
- Do not widen this task into PKI ACL/security model work or unrelated manifest restructuring.

**Verification:**
- Run: `cargo test -p remote-exec-pki --test dev_init_bundle`
- Expect: dev-init bundle generation still writes the expected manifest and pair layout.
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Expect: admin-side dev-init flows still pass if the touched seam is exercised there.

- [ ] Confirm the repeated `KeyPairPaths` construction still matches finding `#20`
- [ ] Add one narrow helper for named pair path construction and update the scoped PKI call sites to use it
- [ ] Reuse the shared helper inside `write_dev_init_bundle` without changing manifest/output semantics
- [ ] Run focused PKI verification, widening to the admin integration test if needed
- [ ] Commit with real code changes only

### Task 3: Extract Shared Rust Logging Helpers For Residual `#21`

**Intent:** Remove the remaining duplicated Rust logging/text helpers while keeping crate-specific logging defaults and existing call sites stable.

**Relevant files/components:**
- Likely modify: `Cargo.toml`
- Likely create: `crates/remote-exec-util/Cargo.toml`
- Likely create: `crates/remote-exec-util/src/lib.rs`
- Likely create: `crates/remote-exec-util/src/logging.rs`
- Likely modify: `crates/remote-exec-broker/Cargo.toml`
- Likely modify: `crates/remote-exec-daemon/Cargo.toml`
- Likely modify: `crates/remote-exec-host/Cargo.toml`
- Likely modify: `crates/remote-exec-broker/src/logging.rs`
- Likely modify: `crates/remote-exec-daemon/src/logging.rs`
- Likely modify: `crates/remote-exec-host/src/logging.rs`
- Existing references: `crates/remote-exec-broker/src/tools/exec.rs`
- Existing references: `crates/remote-exec-broker/src/tools/patch.rs`
- Existing references: `crates/remote-exec-host/src/exec/handlers.rs`

**Notes / constraints:**
- Keep `DEFAULT_FILTER` literals in broker and daemon local to those crates; only the shared bootstrap logic should move.
- Keep the new crate narrow to the approved helpers. Do not migrate unrelated formatting or tracing helpers into it during this batch.
- Preserve current public module visibility unless execution proves a narrower export is safe and still behavior-preserving.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec flows still compile and pass with the shared preview helper.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: broker patch/asset-related code still compiles and passes with unchanged logging/text behavior.
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: daemon crate still compiles and health checks pass with the shared logging bootstrap.

- [ ] Confirm the duplicated `preview_text` and `init_logging` bodies still match residual finding `#21`
- [ ] Add a minimal `remote-exec-util` crate for the shared logging/text helpers and wire it into the workspace
- [ ] Collapse broker, daemon, and host logging modules to thin wrappers or direct shared helper usage
- [ ] Run focused broker/daemon verification for the touched helper consumers
- [ ] Commit with real code changes only

### Task 4: Run The Final Phase E1 Confirmatory Sweep

**Intent:** Re-verify the full E1 surface with fresh evidence and confirm whether any true residual E1 seam remains after Tasks 2 and 3.

**Relevant files/components:**
- Existing references: `docs/CODE_AUDIT_ROUND4.md`
- Existing references: `docs/superpowers/plans/2026-05-12-phase-e1-dedup-seams.md`
- Existing references: `docs/superpowers/plans/2026-05-12-phase-e1-low-risk-rust-dedup.md`
- Existing references: `docs/superpowers/plans/2026-05-12-phase-e1-port-forward-multi-runtime-cleanup.md`

**Notes / constraints:**
- Keep the sweep bounded to Phase E1 findings `#6` through `#21`; do not reopen later-phase cleanup items during this step.
- Use targeted `rg` scans keyed to the known E1 duplicate helper names and shapes, then rely on the focused test commands as the behavioral proof.
- If the sweep is clean, do not create an empty commit. Record the clean result in the execution report only.

**Verification:**
- Run: targeted `rg` scans keyed to E1 findings `#6` through `#21`
- Expect: the duplicate seams are gone, centralized, or intentionally reduced to thin wrappers.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: transfer-related E1 seams still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: asset/image-related E1 seams still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: exec-related E1 seams still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: forwarding-related E1 seams still pass.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon-side forwarding seams still pass.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: the C++ forwarding transport seam still passes.

- [ ] Run the targeted E1 seam scans and classify any remaining hits as clean, intentional wrappers, or true residual issues
- [ ] Run the focused E1 regression commands on the current `HEAD`
- [ ] Stop if the sweep reveals a broad or surprising issue that would require re-planning
- [ ] If the sweep is clean, mark the phase complete without an extra commit

### Task 5: Fix Any Small True Residual E1 Issue Found By Task 4

**Intent:** Leave room for one final scoped cleanup only if the confirmatory sweep finds a genuine E1 issue that can be safely resolved without reopening design.

**Relevant files/components:**
- [confirm exact file paths from Task 4 finding]

**Notes / constraints:**
- Only execute this task if Task 4 finds a true residual E1 seam.
- Keep the fix strictly within Phase E1 scope and avoid opportunistic cleanup outside the identified finding.
- If the issue is larger than a narrow follow-up, stop and return to planning instead of forcing it through this close-out plan.

**Verification:**
- Run: [confirm focused command based on the exact residual issue]
- Expect: the specific residual issue is resolved without regressing the earlier E1 verification set.

- [ ] Confirm the residual issue is real, still in E1 scope, and small enough for a narrow follow-up
- [ ] Implement the scoped fix in code
- [ ] Run the focused verification for that residual issue and re-run any directly affected E1 regression command
- [ ] Commit with real code changes only
