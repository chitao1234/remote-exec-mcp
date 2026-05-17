# Patch Preflight Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Add an internal, user-invisible preflight stage to `apply_patch` so deterministic patch failures are detected before any file mutation while preserving non-transactional execution for unpredictable runtime failures.

**Requirements:**
- Keep the public MCP, CLI, and broker-daemon RPC schemas unchanged; do not add a public dry-run or preflight mode.
- Preserve the `apply_patch` non-transactional contract: if a failure happens during execution after preflight, earlier executed actions remain applied.
- Change the practical semantics so deterministic failures that can be known before mutation are caught up front across the whole ordered patch.
- Keep error wire codes stable: deterministic patch validation failures remain `patch_failed`, sandbox failures remain `sandbox_denied`, and genuine internal invariants remain `internal_error`.
- Keep Rust daemon, broker-local `local`, and C++ daemon behavior aligned where they share the patch contract.
- Avoid broad new abstraction layers; keep the implementation inside the existing patch-engine boundaries.

**Architecture:** The patch pipeline should become `parse -> preflight/plan -> execute`. Preflight resolves and validates every action in order, using an in-memory overlay so later actions see earlier planned patch effects without touching disk. Execution then applies the already-planned actions in order with final bytes prepared where practical; it still handles filesystem races and write-time failures as non-transactional runtime errors.

**Verification Strategy:** Drive the Rust behavior through daemon patch RPC tests and host patch unit tests, then mirror the contract in C++ patch tests. Finish with focused broker-local coverage, docs checks by inspection, `cargo fmt --all --check`, relevant Rust tests, and relevant C++ checks for touched C++ code.

**Assumptions / Open Questions:**
- The implementation may choose whether the Rust preflight planner lives in a new `preflight.rs` module or replaces/expands `verify.rs`; the boundary must stay patch-local.
- The exact overlay keying should be confirmed against existing host path normalization/comparison helpers during implementation, especially for Windows and paths containing lexical parent segments.
- No cross-process locking is required. Races after preflight are explicitly outside the deterministic-failure guarantee.

---

### Task 1: Add Rust Preflight Contract Tests

**Intent:** Lock in the new user-invisible semantics before changing the Rust patch engine: deterministic failures anywhere in the ordered patch should reject the request before mutation.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/tests/patch_rpc.rs`
- Likely modify: `crates/remote-exec-host/src/patch/engine.rs`
- Likely modify: `crates/remote-exec-host/src/patch/parser.rs`
- Existing references: `crates/remote-exec-host/src/patch/mod.rs`
- Existing references: `crates/remote-exec-host/src/patch/verify.rs`

**Notes / constraints:**
- Existing tests that assert deterministic later failures leave earlier mutations applied should be updated to assert no mutation.
- Cover both filesystem validation failures and hunk/content validation failures.
- Add ordered-dependency coverage so preflight does not reject valid patches that depend on earlier actions in the same patch.
- Keep tests focused on observable behavior; do not couple them to internal planner types.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test patch_rpc`
- Run: `cargo test -p remote-exec-host patch::`
- Expect: new or updated tests fail before implementation and describe the intended preflight behavior clearly.

- [ ] Inspect the current deterministic partial-mutation tests and decide which should become preflight tests.
- [ ] Add or update Rust daemon tests for later missing delete target, directory delete target, non-UTF-8 source when autodetect is disabled, unmatched update hunk, parent path conflict, and later sandbox denial.
- [ ] Add valid ordered-dependency tests for add-then-update, update-then-delete, move-then-update-destination, and add-then-delete where appropriate.
- [ ] Add focused host unit coverage if a pure planner seam already exists or becomes obvious while inspecting the code.
- [ ] Run focused Rust tests and confirm they fail only for the missing preflight implementation.
- [ ] Commit.

### Task 2: Implement Rust Preflight Planning With Virtual Overlay

**Intent:** Replace the current parse-and-execute loop with a two-phase Rust host pipeline that plans all deterministic effects before the first write.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/patch/mod.rs`
- Likely modify: `crates/remote-exec-host/src/patch/verify.rs`
- Likely create: `crates/remote-exec-host/src/patch/preflight.rs`
- Likely modify: `crates/remote-exec-host/src/patch/text_codec.rs`
- Likely modify: `crates/remote-exec-host/src/patch/engine.rs`
- Existing references: `crates/remote-exec-host/src/host_path.rs`
- Existing references: `crates/remote-exec-host/src/path_compare.rs`

**Notes / constraints:**
- Preflight must parse once, resolve paths once per action, and produce execution-ready planned actions.
- Update actions should have hunk matching and output encoding completed during preflight, not during execution.
- Maintain a virtual overlay keyed by resolved patch paths so later actions see earlier planned adds, deletes, updates, and moves.
- Model implicit parent directories enough to catch deterministic parent/file conflicts caused by earlier planned actions or current filesystem state.
- Keep execution ordered and non-transactional; do not add rollback or all-or-nothing commit behavior.
- Preserve current text behavior: update line-ending preservation, `*** End of File`, matcher normalization, and optional target encoding autodetection.

**Verification:**
- Run: `cargo test -p remote-exec-host patch::`
- Run: `cargo test -p remote-exec-daemon --test patch_rpc`
- Expect: Rust patch engine catches deterministic failures before mutation and preserves existing successful patch behavior.

**Demonstration snippet (illustrative only):**
```rust
enum PlannedPatchAction {
    Add { path: PathBuf, content: Vec<u8>, summary_path: String },
    Delete { path: PathBuf, summary_path: String },
    Update {
        source_path: PathBuf,
        destination_path: PathBuf,
        content: Vec<u8>,
        summary_path: String,
        remove_source: bool,
    },
}
```

- [ ] Inspect the current `ResolvedAction` and `execute_actions` seam and choose whether to evolve `verify.rs` or introduce `preflight.rs`.
- [ ] Introduce a planned-action representation that contains final write bytes for add/update actions and only execution-time filesystem operations for delete/move cleanup.
- [ ] Add a virtual overlay for planned file presence/deletion and enough planned parent-directory awareness to catch deterministic path conflicts.
- [ ] Move update source reads, text decoding, hunk application, and output encoding into preflight while keeping execution race-safe.
- [ ] Ensure sandbox checks still use resolved write targets and still return `sandbox_denied` before any mutation.
- [ ] Run focused Rust verification.
- [ ] Commit.

### Task 3: Preserve Broker-Local Behavior And Public Error/Text Contracts

**Intent:** Confirm the Rust host change reaches both Rust daemon RPC and broker-host `local` patch behavior without changing the public tool or CLI surface.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Likely inspect: `crates/remote-exec-broker/src/tools/patch.rs`
- Likely inspect: `crates/remote-exec-broker/src/local/backend.rs`
- Likely inspect: `crates/remote-exec-broker/src/bin/remote_exec.rs`
- Existing references: `crates/remote-exec-proto/src/public/assets.rs`
- Existing references: `crates/remote-exec-proto/src/rpc/patch.rs`

**Notes / constraints:**
- Do not add fields to `ApplyPatchInput`, `PatchApplyRequest`, or `PatchApplyResponse`.
- Successful calls should keep returning text output only through MCP.
- Error wrapping and correlation should remain broker-owned and unchanged except for improved daemon/local error messages.
- The broker stub daemon does not need to implement the full preflight engine; local-target integration should cover broker-host use of the Rust host runtime.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: public apply-patch output shape remains stable and broker-local preflight behavior matches daemon behavior.

- [ ] Inspect broker apply-patch routing and confirm no schema or tool-registration changes are needed.
- [ ] Add or adjust broker-local tests only where they prove the shared Rust host preflight behavior reaches `target: "local"`.
- [ ] Confirm CLI `apply-patch` needs no new option, output mode, or JSON shape.
- [ ] Run focused broker tests.
- [ ] Commit.

### Task 4: Align The C++ Patch Engine With The Preflight Contract

**Intent:** Bring the standalone C++11 daemon patch engine into parity for the shared patch behavior: deterministic failures are caught before mutation, but runtime execution remains non-transactional.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/patch_engine.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/include/patch_engine.h` only if internal result/helper shape requires a narrow declaration change.
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_patch.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`

**Notes / constraints:**
- Keep the C++ public HTTP route and JSON response shape unchanged.
- Keep C++11 and Windows XP-compatible build constraints in mind; avoid Rust-inspired abstractions that do not fit the C++ daemon.
- The C++ matcher is not identical to the Rust matcher in every tolerance detail today; this task should preserve existing C++ successful behavior while adding equivalent preflight staging.
- Mirror the Rust ordered virtual overlay semantics for add/update/delete/move dependencies within one patch.
- Keep sandbox denial surfaced as `sandbox_denied` from route handling where applicable.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-patch`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
- Expect: C++ deterministic later failures do not mutate earlier actions, valid ordered dependencies still work, and XP-compatible compile/test gates remain healthy.

- [ ] Inspect the C++ parser, path resolution, patch action execution, and route error handling seams.
- [ ] Add C++ tests mirroring the key Rust preflight behavior and ordered virtual dependency cases.
- [ ] Introduce a C++ preflight/planned-action stage local to `patch_engine.cpp`.
- [ ] Keep action execution ordered and non-transactional for write-time failures after preflight.
- [ ] Run focused C++ verification, including the XP-compatible gate if available in the environment.
- [ ] Commit.

### Task 5: Update Live Contract Documentation

**Intent:** Make the user-facing contract describe the new preflighted-but-still-non-transactional semantics without implying atomicity.

**Relevant files/components:**
- Likely modify: `README.md`
- Likely modify: `skills/using-remote-exec-mcp/SKILL.md`
- Likely modify: `crates/remote-exec-daemon-cpp/README.md`
- Likely inspect: `configs/*.example.toml`

**Notes / constraints:**
- Do not edit historical planning/audit docs outside this plan unless they are explicitly requested later.
- Config examples probably do not need changes unless implementation changes target encoding autodetect wording.
- Use precise wording: deterministic failures are preflighted before writes, but filesystem races and write/remove failures during execution can still leave earlier actions applied.

**Verification:**
- Run: `rg -n "non-transactional|apply_patch|preflight" README.md skills/using-remote-exec-mcp/SKILL.md crates/remote-exec-daemon-cpp/README.md configs`
- Expect: all live docs agree on the same patch semantics and do not promise rollback or atomicity.

- [ ] Update `README.md` `apply_patch` notes and reliability/trust text if needed.
- [ ] Update `skills/using-remote-exec-mcp/SKILL.md` guidance for remote tool users.
- [ ] Update the C++ daemon README contract note.
- [ ] Inspect config examples and leave them unchanged unless wording is directly affected.
- [ ] Run the documentation consistency search.
- [ ] Commit.

### Task 6: Final Focused And Cross-Cutting Verification

**Intent:** Confirm the full Rust and C++ patch contract is stable, formatted, and aligned with the repository’s public-surface expectations.

**Relevant files/components:**
- Existing references: `crates/remote-exec-host/src/patch/`
- Existing references: `crates/remote-exec-daemon/tests/patch_rpc.rs`
- Existing references: `crates/remote-exec-broker/tests/mcp_assets.rs`
- Existing references: `crates/remote-exec-daemon-cpp/tests/test_patch.cpp`
- Existing references: live docs listed in Task 5

**Notes / constraints:**
- Run targeted tests before broader gates.
- Do not run tests under Wine unless explicitly requested.
- If a Windows XP or Windows GNU gate is unavailable locally, record that honestly and run the strongest available substitute.

**Verification:**
- Run: `cargo test -p remote-exec-host patch::`
- Run: `cargo test -p remote-exec-daemon --test patch_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp` when the local toolchain supports it.
- Run: `cargo fmt --all --check`
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings` if the implementation touched shared Rust behavior broadly enough to warrant the full lint gate.
- Expect: all focused checks pass; any unavailable platform gate is called out with the exact reason.

- [ ] Run the Rust host, daemon, broker, and C++ focused checks.
- [ ] Run formatting and the appropriate lint gate.
- [ ] Search for stale wording or tests that still describe deterministic partial mutation as expected behavior.
- [ ] Inspect `git diff` for accidental schema, config, or unrelated docs churn.
- [ ] Commit the final verification/doc cleanup changes.
