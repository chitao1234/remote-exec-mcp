# Phase E5 Smaller Cleanups And Final Sweep Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Finish the remaining live Audit Round 4 Phase E work by landing the smaller host/C++/PKI cleanups and then running a final confirmatory sweep across the full Phase E1-E5 surface.

**Requirements:**
- Cover the remaining live Phase E5 findings from `docs/CODE_AUDIT_ROUND4.md`: `#31`, `#47`, `#48`, `#49`, `#50`, `#51`, and `#52`.
- Resolve the staged-summary overlap explicitly: `#27` was completed in Phase E3, and `#39`, `#41`, `#42`, and `#43` were completed in Phase E4. Revalidate them only during the final sweep; do not reopen them as Phase E5 implementation work.
- Keep the work plan-based and commit after each real task only when that task has actual code changes; do not create empty commits.
- Preserve the public MCP tool contract, broker/daemon wire behavior, PKI CLI behavior, current POSIX and Windows build entry points, and existing test expectations unless a touched regression test already documents a different intended behavior.
- Include a final Round 4 Phase E* sweep task that checks the entire staged Phase E surface (`#1` through `#52`) before declaring the audit series closed.
- Do not widen this phase into new architecture work outside the identified findings, even if nearby files make additional cleanup tempting.

**Architecture:** Execute Phase E5 as three medium-sized cleanup batches followed by a close-out sweep. First, tighten the remaining Rust host seams by replacing ad-hoc port-forward test JSON with typed helpers, moving PTY-generic terminal filtering out of a Windows-only file, and removing the last leaked Windows-only Unix-shell parameter. Second, clean up the standalone C++ build graph by factoring repeated source inventories and bringing the GNU make Windows-native path up to the same test-surface intent as the existing XP/MSVC paths without broadening platform scope. Third, trim the final PKI write-path duplication and move Linux-only inotify support fully out of production library code. Finish with a final Phase E* sweep that uses the earlier Phase E plan artifacts plus the audit itself as the checklist, reruns representative regression/build coverage, and only allows one narrow residual follow-up if the sweep finds a true remaining issue.

**Verification Strategy:** Verify each batch with the narrowest commands that actually exercise the touched seam, then widen only where the cleanup crosses runtime or platform boundaries. The Rust host batch should use `remote-exec-host` tests plus daemon-side exec/port-forward integration coverage and a Windows target compile gate because it touches Windows-only PTY code. The C++ build batch should prove the refactored source inventory still works on POSIX and remains structurally aligned with BSD make and Windows-native/MSVC target shapes. The PKI batch should use the PKI crate tests and the admin CLI integration seam because the cleanup touches write orchestration and test-only filesystem monitoring. The final sweep should combine targeted `rg` scans keyed to findings `#1` through `#52`, representative cross-phase regression commands, and the final quality gate from `README.md`.

**Assumptions / Open Questions:**
- For `#31`, the lower-risk direction is to keep the new typed frame helpers test-scoped inside `remote-exec-host` rather than moving more constructor helpers into `remote-exec-proto`.
- For `#49`, a small shared PTY-filter module under `crates/remote-exec-host/src/exec/session/` is preferred over leaving PTY-generic code inside `windows.rs`, but the final file name should be confirmed during execution.
- For `#48`, actual execution of a new GNU make Windows-native test target may require a native Windows environment. If that is unavailable during implementation, confirm the target wiring structurally and rely on the existing MSVC native test surface plus the final Windows Rust compile gate.
- For `#52`, moving Linux inotify FFI out of `src/write.rs` may require a small test-support module under `crates/remote-exec-pki/tests/`; keep production APIs unchanged and confirm the narrowest clean placement during execution.

---

### Task 1: Save The Phase E5 Plan

**Intent:** Create the tracked Phase E5 merged design + execution artifact before implementation begins.

**Relevant files/components:**
- Likely modify: `docs/superpowers/plans/2026-05-13-phase-e5-smaller-cleanups-and-final-sweep.md`

**Notes / constraints:**
- The repo already tracks planning artifacts under `docs/superpowers/plans/`.
- Do not start Phase E5 code changes until the user reviews and approves this plan artifact.

**Verification:**
- Run: `test -f docs/superpowers/plans/2026-05-13-phase-e5-smaller-cleanups-and-final-sweep.md`
- Expect: the plan file exists at the tracked path.

- [ ] Add the merged Phase E5 plan at the tracked path
- [ ] Confirm the plan keeps the live Phase E5 scope limited to `#31`, `#47`, `#48`, `#49`, `#50`, `#51`, and `#52`, with prior overlap items called out as already completed
- [ ] Confirm the plan includes a final Round 4 Phase E* sweep task
- [ ] Verify the plan file exists
- [ ] Commit

### Task 2: Rust Host Smaller Cleanups

**Intent:** Finish the remaining `remote-exec-host` maintainability cleanup by replacing raw port-forward test frame JSON with typed helpers, moving PTY-generic terminal filtering to the right module boundary, and removing the dead Unix shell parameter.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-host/src/exec/session/mod.rs`
- Likely modify: `crates/remote-exec-host/src/exec/session/live.rs`
- Likely modify: `crates/remote-exec-host/src/exec/session/windows.rs`
- Likely create: `crates/remote-exec-host/src/exec/session/[confirm shared PTY-filter module name].rs`
- Likely modify: `crates/remote-exec-host/src/exec/shell/unix.rs`
- Existing references: `crates/remote-exec-proto/src/port_tunnel.rs`
- Existing references: `crates/remote-exec-daemon/tests/exec_rpc.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`

**Notes / constraints:**
- Cover findings `#31`, `#49`, and `#50`.
- Keep the new port-forward frame helpers test-scoped; do not widen the public proto API just to avoid local test helper code.
- Preserve the current PTY filtering behavior, CRLF normalization, and Windows device-status-report handling exactly when moving `TerminalOutputState` / `TerminalOutputPerformer`.
- Removing `_windows_posix_root` from the Unix shell path should shrink signatures rather than add a new dummy wrapper seam.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Expect: host-local port-forward and PTY/session tests still pass after the helper/module cleanup.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: daemon exec behavior still passes with the session-module reshaping.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: host-backed port-forward behavior still passes with the typed frame helper cleanup.
- Run: `cargo check -p remote-exec-host --all-targets --all-features --target x86_64-pc-windows-gnu`
- Expect: the moved PTY-filter code still compiles for the Windows target.

- [ ] Inspect the current host test/helper seams and confirm the narrowest helper/module placement
- [ ] Replace inline port-forward test frame JSON with typed helper construction that stays test-scoped
- [ ] Move PTY-generic terminal filter code out of `windows.rs` and remove the dead Unix shell parameter without changing behavior
- [ ] Run focused host + daemon verification, including the Windows target compile gate
- [ ] Commit with real code changes only

### Task 3: C++ Build Inventory And Windows-Native Target Parity

**Intent:** Remove the repeated standalone C++ source-list expansion and add the missing GNU make Windows-native test-target surface without changing the intended platform split.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/mk/sources.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/windows-native.mk`
- Likely modify: `crates/remote-exec-daemon-cpp/GNUmakefile`
- Likely modify: `crates/remote-exec-daemon-cpp/mk/posix.mk`
- Existing references: `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
- Existing references: `crates/remote-exec-daemon-cpp/NMakefile`

**Notes / constraints:**
- Cover findings `#47` and `#48`.
- Factor shared source inventory through a small number of named lists; do not bury the daemon’s real compile inputs behind a broad new abstraction layer.
- Keep BSD make POSIX-only and keep GNU make / XP / MSVC native target names aligned only where they intentionally support the same build path.
- The Windows-native GNU make target should mirror the existing native/XP test-group intent as closely as the current make split allows, not create a new platform matrix.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the POSIX daemon and host-test source inventory still builds and passes after the shared-list cleanup.
- Run: `bmake -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the BSD make entry point still accepts the factored source inventory without GNU-make-only regressions.
- Run: `nmake /f crates\\remote-exec-daemon-cpp\\NMakefile check-msvc-native`
- Expect: when a native Windows MSVC environment is available, the native Windows test surface remains aligned with the intended target set.
- Run: [confirm the new GNU make Windows-native test target name during implementation]
- Expect: when a native Windows GNU make environment is available, the new target builds and runs the intended native Windows tests.

- [ ] Confirm the smallest shared source-list extraction that removes the repeated `BASE_SRCS` expansions cleanly
- [ ] Add the missing GNU make Windows-native test-target surface and keep the target naming aligned with existing Windows-native intent
- [ ] Verify POSIX and BSD make entry points still work, and confirm Windows-native/MSVC parity as far as the environment allows
- [ ] Document any environment-conditional verification results explicitly during execution
- [ ] Commit with real code changes only

### Task 4: PKI Write-Path And Test-Support Cleanup

**Intent:** Finish the remaining PKI cleanups by removing the redundant existence guard in `write_text_file` and moving Linux-only inotify support out of production library code.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-pki/src/write.rs`
- Likely create: `crates/remote-exec-pki/tests/support/inotify.rs`
- Likely modify: `crates/remote-exec-pki/tests/dev_init_bundle.rs`
- Existing references: `crates/remote-exec-pki/tests/ca_reuse.rs`
- Existing references: `crates/remote-exec-admin/tests/dev_init.rs`

**Notes / constraints:**
- Cover findings `#51` and `#52`.
- Preserve the current overwrite semantics and error text contract driven by `validate_output_paths`; the cleanup should remove duplicate checking, not subtly change write behavior.
- Keep Linux inotify FFI test-only. Do not expose new production PKI APIs just to support the tests.
- Preserve the current Windows private-key ACL hardening behavior untouched.

**Verification:**
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI unit and integration coverage still passes after moving test support and simplifying `write_text_file`.
- Run: `cargo test -p remote-exec-pki --test dev_init_bundle`
- Expect: the end-to-end dev-init bundle write path still passes with the new test-support layout.
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Expect: admin CLI flows that rely on PKI write helpers still pass.

- [ ] Confirm the narrowest test-support layout for moving the Linux inotify helper out of `src/write.rs`
- [ ] Remove the redundant existence check and relocate the Linux inotify helper to test-only support
- [ ] Run focused PKI and admin verification
- [ ] Commit with real code changes only

### Task 5: Run The Final Round 4 Phase E* Sweep

**Intent:** Re-verify the entire staged Round 4 Phase E surface with fresh evidence and confirm whether any true residual issue remains after the live Phase E5 cleanup tasks.

**Relevant files/components:**
- Existing references: `docs/CODE_AUDIT_ROUND4.md`
- Existing references: `docs/superpowers/plans/2026-05-12-phase-e1-closeout.md`
- Existing references: `docs/superpowers/plans/2026-05-12-phase-e2-split-god-modules-and-headers.md`
- Existing references: `docs/superpowers/plans/2026-05-13-phase-e3-boundaries-and-lifecycle-wrappers.md`
- Existing references: `docs/superpowers/plans/2026-05-13-phase-e4-struct-sprawl-and-ad-hoc-handlers.md`
- Existing references: `docs/superpowers/plans/2026-05-13-phase-e5-smaller-cleanups-and-final-sweep.md`

**Notes / constraints:**
- Keep the sweep bounded to the staged Round 4 Phase E findings `#1` through `#52`; do not reopen unrelated audit categories or invent new cross-cutting refactors.
- Use targeted `rg` scans keyed to the resolved smell patterns from each phase, then rely on representative regression/build commands and the final quality gate as behavioral proof.
- If the sweep is clean, do not create an empty commit. Record the clean result in the execution report only.
- If the sweep reveals a broad or surprising issue that would require rethinking the phase breakdown, stop and return to planning instead of forcing through a speculative patch.

**Verification:**
- Run: targeted `rg` scans keyed to findings `#1` through `#52`
- Expect: the original code-smell shapes are gone, intentionally centralized, or reduced to thin documented wrappers.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec/session seams from earlier phases still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker transfer/path seams from earlier phases still pass.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding seams from earlier phases still pass.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: daemon exec-host seams still pass on the final `HEAD`.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon forwarding seams still pass on the final `HEAD`.
- Run: `cargo test -p remote-exec-host`
- Expect: host-local cleanup seams and earlier host behavior still pass together.
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI cleanup seams and earlier certificate/write behavior still pass together.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the standalone C++ POSIX build/test surface still passes on the final `HEAD`.
- Run: `cargo test --workspace`
- Expect: the full Rust workspace regression suite passes.
- Run: `cargo fmt --all --check`
- Expect: formatting is clean on the final `HEAD`.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: the full Rust lint gate passes with no warnings promoted to errors.
- Run: `cargo check --workspace --all-targets --all-features --target x86_64-pc-windows-gnu`
- Expect: the final Rust codebase still compiles for the Windows GNU target after the full Phase E series.

- [ ] Run the targeted Phase E* seam scans and classify any remaining hits as clean, intentional wrappers, or true residual issues
- [ ] Run the representative cross-phase regression/build commands on the current `HEAD`
- [ ] Run the final Rust quality gate and record any environment-conditional C++ or Windows-native verification separately
- [ ] Stop if the sweep reveals a broad or surprising issue that would require re-planning
- [ ] If the sweep is clean, mark Round 4 Phase E complete without an extra commit

### Task 6: Fix Any Small True Residual Phase E Issue Found By Task 5

**Intent:** Leave room for one final scoped cleanup only if the Phase E* sweep finds a genuine remaining issue that can be safely fixed without reopening the architecture.

**Relevant files/components:**
- [confirm exact file paths from Task 5 finding]

**Notes / constraints:**
- Only execute this task if Task 5 finds a true residual issue.
- Keep the fix strictly within Round 4 Phase E scope and avoid opportunistic cleanup outside the identified finding.
- If the issue is larger than a narrow follow-up, stop and return to planning instead of forcing it through this close-out plan.

**Verification:**
- Run: [confirm focused command based on the exact residual issue]
- Expect: the specific residual issue is resolved without regressing the directly affected earlier-phase verification.

- [ ] Confirm the residual issue is real, still inside Phase E scope, and small enough for a narrow follow-up
- [ ] Implement the scoped fix in code
- [ ] Run the focused verification for that residual issue and re-run any directly affected sweep command
- [ ] Commit with real code changes only
