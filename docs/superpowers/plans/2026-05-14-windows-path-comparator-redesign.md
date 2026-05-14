# Windows Path Comparator Redesign Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Replace the current key-based Windows path comparison design with syntax-only shared path helpers plus native local comparator backends, landing Rust first and then aligning the C++ daemon.

**Requirements:**
- Keep `remote-exec-proto::path` limited to syntax and lexical normalization concerns; it must not expose authoritative filesystem comparison semantics.
- Narrow all broker logic for `target != local` to syntax-only behavior for every platform, including Windows and POSIX targets.
- Keep obvious remote duplicate rejection only when normalized syntax strings are exactly equal; do not case-fold or claim native filesystem equality for remote targets.
- Move authoritative local Windows comparison to a native backend using Win32 APIs through `windows-sys` in Rust and Win32 APIs in the C++ daemon.
- Stop representing Windows comparison as a precomputed lowered/folded string key; use comparator operations instead.
- Do not preserve fake “Windows sandbox semantics on non-Windows hosts”; Windows-native comparison tests should be authoritative only on Windows-capable runs.
- Keep the public tool surface and wire formats unchanged; this is an internal boundary and correctness redesign.

**Architecture:** Shared path helpers stay in `remote-exec-proto` and become explicitly syntax-only: absolute-path detection, separator and drive-shape normalization, basename extraction, join behavior, and lexical normalization. Native comparison moves to runtime-local modules: Rust host-side comparator helpers for broker-local and daemon-local behavior, and a matching internal comparator layer in the C++ daemon. Sandbox containment, local same-path checks, and local prefix checks must call comparator operations directly; broker remote-target logic may only use syntax shaping and exact normalized-text equality for obvious duplicates.

**Verification Strategy:** Land the Rust redesign in focused slices, rerunning targeted `remote-exec-proto`, `remote-exec-host`, broker transfer, and daemon transfer/sandbox coverage after each task, plus a Windows GNU compile gate for Rust local Windows code. Then land the C++ alignment and rerun `make -C crates/remote-exec-daemon-cpp check-posix` plus `make -C crates/remote-exec-daemon-cpp all-windows-xp`. Finish with a confirmatory sweep that the removed key-based APIs are gone from active call sites and that broker remote checks remain syntax-only.

**Assumptions / Open Questions:**
- Confirm during implementation whether the final syntax-only helper name should remain `normalize_for_system` or be renamed to something more explicit such as `normalize_syntax_for_policy`.
- Confirm whether the Rust sandbox runtime should move wholesale into `remote-exec-host` or whether a thin `remote-exec-proto` types-only facade is preferable; the approved boundary requires comparator semantics to live outside `proto`.
- Confirm the exact final host-side module names for Rust comparator and sandbox runtime helpers; names such as `path_compare.rs` and `sandbox_runtime.rs` are guidance, not required filenames.
- Confirm whether any broker-local path checks outside transfer handling still depend on shared comparison helpers and should migrate in the same pass once the seam is inspected.

---

### Task 1: Make Shared Rust Path Helpers Syntax-Only And Narrow Broker Remote Preflight

**Intent:** Remove filesystem-comparison semantics from `remote-exec-proto::path` and rework broker remote endpoint handling so all `target != local` path logic is syntax-only.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/path.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Likely modify: `crates/remote-exec-broker/tests/mcp_transfer.rs`
- Existing references: `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- Existing references: `crates/remote-exec-proto/src/public.rs` (only if test fixtures or comments need shape clarification)

**Notes / constraints:**
- Remove or retire `comparison_key_for_policy`, `same_path_for_policy`, and `path_text_eq` from the shared path API.
- Preserve syntax helpers for remote path shaping: absolute-path detection, basename extraction, join behavior, and Windows `/c/...` / `/cygdrive/...` translation.
- Broker remote duplicate handling may only reject paths that are exactly equal after syntax normalization; it must not case-fold or imply filesystem truth.
- Broker local checks may temporarily keep existing behavior until the Rust native comparator is in place, but remote code must stop depending on shared equality helpers in this task.

**Verification:**
- Run: `cargo test -p remote-exec-proto [confirm exact path-related test selector or run the crate tests if cheaper than curating selectors]`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: shared path syntax tests and broker transfer path-shaping coverage pass, with remote duplicate behavior narrowed to exact normalized-syntax equality.

- [ ] Inspect the current shared path API and confirm the final syntax-only helper surface to preserve in `remote-exec-proto`.
- [ ] Remove shared comparison-key semantics and add any syntax-only helper needed to keep broker code readable without reintroducing native comparison claims.
- [ ] Migrate broker remote transfer endpoint logic to syntax-only equality and syntax-only destination shaping for all remote platforms.
- [ ] Update broker and shared path tests to reflect the approved remote-path boundary, keeping only obvious normalized-text duplicate rejection.
- [ ] Run focused Rust verification.
- [ ] Commit.

### Task 2: Add Rust Native Comparator Operations And Migrate Non-Sandbox Local Call Sites

**Intent:** Introduce a Rust host-side comparator API with a Windows-native backend, then migrate local path-equality and prefix checks that are not part of sandbox enforcement yet.

**Relevant files/components:**
- Likely create: `crates/remote-exec-host/src/path_compare.rs`
- Likely modify: `crates/remote-exec-host/src/lib.rs`
- Likely modify: `crates/remote-exec-host/src/host_path.rs`
- Likely modify: `crates/remote-exec-host/src/patch/verify.rs`
- Likely modify: `crates/remote-exec-host/src/exec/shell/windows.rs`
- Existing references: `Cargo.toml`
- Existing references: `crates/remote-exec-host/Cargo.toml`

**Notes / constraints:**
- Comparator operations should work on `Path` / `OsStr` seams where practical, not on lossy UTF-8 comparison keys.
- Windows comparison must use Win32-native behavior through `windows-sys`, with UTF-16 component comparisons via `CompareStringOrdinal(..., TRUE)`.
- POSIX comparison remains exact component equality after existing lexical normalization.
- Remove non-Windows tests that currently claim to prove native Windows Unicode comparison behavior; replace them with Windows-only native tests or syntax-only tests as appropriate.

**Verification:**
- Run: `cargo test -p remote-exec-host [confirm exact selectors for host_path, patch verify, and Windows shell dedup coverage]`
- Run: `cargo check --workspace --all-targets --all-features --target x86_64-pc-windows-gnu`
- Expect: host comparator call sites pass on the current host, and Windows-target compilation succeeds with the native comparator backend wired in.

- [ ] Inspect the current local path equality and prefix call sites and confirm the comparator API surface needed in `remote-exec-host`.
- [ ] Introduce the Rust native comparator module with explicit operations such as component equality, path equality, and prefix/within checks.
- [ ] Migrate local non-sandbox path consumers in host-path, patch verification, and Windows shell candidate dedup to the comparator API.
- [ ] Update or replace tests so Windows-native comparison claims are exercised only on Windows-capable runs, while cross-platform tests stay syntax-only.
- [ ] Run focused host verification and the Windows compile gate.
- [ ] Commit.

### Task 3: Move Rust Sandbox Containment Semantics Out Of Proto And Finish Local Runtime Migration

**Intent:** Re-home authoritative sandbox compile/authorize behavior into the Rust local runtime so native comparator semantics own local containment checks.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/sandbox.rs`
- Likely modify or shrink: `crates/remote-exec-proto/src/sandbox/authorize.rs`
- Likely modify: `crates/remote-exec-proto/src/sandbox/path_utils.rs`
- Likely create: `crates/remote-exec-host/src/sandbox_runtime.rs`
- Likely modify: `crates/remote-exec-host/src/state.rs`
- Likely modify: `crates/remote-exec-host/src/exec/support.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/import.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/export/prepare.rs`
- Likely modify: `crates/remote-exec-broker/src/startup.rs`
- Likely modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Existing references: `crates/remote-exec-broker/src/local_backend.rs`

**Notes / constraints:**
- `remote-exec-proto` may keep sandbox schema/types, but authoritative local compile/authorize behavior must live in runtime code that can call native comparators.
- Broker-local and daemon-local sandbox enforcement must route through the Rust host/runtime layer instead of shared `proto` comparison helpers.
- Windows sandbox semantics on non-Windows hosts are not a supported behavior target after this redesign; remove or narrow tests that currently assert otherwise.
- Keep the public sandbox config shape unchanged.

**Verification:**
- Run: `cargo test -p remote-exec-host [confirm exact sandbox-related selectors or run the crate tests if refactor breadth makes selectors less useful]`
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: local sandbox enforcement and transfer path authorization still behave correctly, with runtime-local comparison semantics instead of shared key-based behavior.

- [ ] Inspect the current sandbox compile/authorize seam and confirm how much of the runtime should move from `remote-exec-proto` into `remote-exec-host`.
- [ ] Introduce host-owned sandbox runtime helpers that compile and authorize local paths using the native comparator API where needed.
- [ ] Rewire broker-local and daemon-local sandbox callers to the host/runtime seam and remove the shared comparison dependency from `remote-exec-proto`.
- [ ] Update sandbox and transfer tests to reflect the approved local-vs-remote boundary and remove fake non-Windows Windows-native assertions.
- [ ] Run focused runtime, daemon, and broker verification.
- [ ] Commit.

### Task 4: Align The C++ Daemon With The Same Syntax-Only Path Layer And Native Comparator Model

**Intent:** Redesign the C++ daemon to stop using lowered comparison keys and instead use explicit comparator operations, mirroring the Rust boundary after the Rust stage is stable.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/path_policy.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/path_policy.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/include/path_compare.h`
- Likely create: `crates/remote-exec-daemon-cpp/src/path_compare.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/filesystem_sandbox.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_sandbox.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/src/config.cpp`

**Notes / constraints:**
- `path_policy` should remain syntax-only after this task: path style, absolute-path syntax, drive-shape translation, and join behavior.
- Remove `path_policy_comparison_key` and `same_path_for_policy` as comparison primitives.
- Windows comparison must use Win32-native UTF-16 component comparisons through `CompareStringOrdinal(..., TRUE)`, not `CharLowerBuffW`-generated string keys.
- POSIX-host tests should no longer claim to prove native Windows comparison semantics; keep syntax coverage portable and reserve authoritative Windows-native behavior for Windows-capable runs.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp all-windows-xp`
- Expect: C++ syntax and sandbox tests pass on POSIX, and the Windows XP compile gate succeeds with the comparator redesign in place.

- [ ] Inspect the current C++ path-policy and sandbox comparison seams and confirm the final comparator API shape for the daemon.
- [ ] Introduce the C++ comparator module and migrate local equality/prefix/within logic away from lowered comparison keys.
- [ ] Remove key-based Windows comparison helpers from `path_policy` and update sandbox containment to component-wise comparator operations.
- [ ] Update C++ tests so syntax-only assertions remain portable and Windows-native comparison assertions are bounded to appropriate environments.
- [ ] Run focused C++ verification.
- [ ] Commit.

### Task 5: Final Confirmatory Sweep Across Rust And C++ Path Semantics

**Intent:** Perform a final repo-level sweep to confirm the redesign boundaries are actually enforced and no key-based Windows comparison seam remains active.

**Relevant files/components:**
- Existing references: `crates/remote-exec-proto/src/path.rs`
- Existing references: `crates/remote-exec-host/src/`
- Existing references: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Existing references: `crates/remote-exec-daemon-cpp/src/`
- Existing references: `Cargo.toml`

**Notes / constraints:**
- Confirm that broker remote path handling is syntax-only for all platforms after the redesign.
- Confirm that any remaining Windows-native comparison logic is runtime-local and does not leak back into shared `proto` APIs.
- Confirm whether the `caseless` dependency becomes removable after the Rust stage; if so, remove it in the same final sweep.
- Finish with a codebase search for removed APIs and stale helper names before declaring the redesign complete.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Run: `cargo check --workspace --all-targets --all-features --target x86_64-pc-windows-gnu`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp all-windows-xp`
- Expect: targeted Rust and C++ coverage pass, the Windows compile gates stay green, and a final grep confirms key-based Windows comparison helpers are gone from active implementations.

- [ ] Inspect the final tree for stale shared comparison-key APIs, fake Windows-native test assertions on non-Windows, and any broker remote path logic that still implies filesystem truth.
- [ ] Remove leftover dependencies or helpers that are no longer needed after the redesign, including `caseless` if the Rust stage made it dead.
- [ ] Run the focused Rust and C++ verification commands and capture any remaining boundary regressions.
- [ ] Perform a final confirmatory sweep for code-search holdouts before marking the redesign complete.
- [ ] Commit.
