# Section 9 Structure And Naming Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Clean up the verified section-9 naming and module-structure inconsistencies that genuinely hurt discoverability, while rejecting stale or speculative audit items and keeping behavior unchanged.

**Requirements:**
- Limit this pass to the validated section-9 items from `docs/code-quality-audit-2026-05-16.md`.
- Accept the Rust structure items `9.1`, `9.2`, `9.3`, `9.7`, and a narrowed `9.8`.
- Accept the low-risk C++ cleanliness items `9.5` and `9.6`.
- Reject `9.4` as written because the cited `port_forward/active.rs` file no longer exists; do not invent a replacement cleanup just to satisfy the stale audit text.
- Keep the public tool surface, broker-daemon wire format, daemon startup behavior, and C++ daemon behavior unchanged.
- Do not let `9.8` turn into the larger backend-abstraction redesign from section 6; this pass is about naming and placement clarity, not a new polymorphism model.

**Architecture:** The right scope for section 9 is structural hygiene rather than behavioral refactoring. In Rust, that means regrouping obviously related broker-local modules, moving shared tunnel-open helpers into a module whose name matches its responsibility, renaming or relocating the host exec helper module so its exported surface reads coherently, and separating generic HTTP-serving helpers from TLS-specific feature dispatch. In C++, the right scope is purely consistency: converge the cited constant names toward one local style and align the include-guard outliers with the project’s prevailing `#pragma once` convention.

**Verification Strategy:** Use targeted Rust tests and compile gates around the touched broker/daemon/host seams, plus the relevant POSIX C++ checks after the header and constant-name cleanup. Finish with format/lint checks only if the moved modules or renamed paths create new style fallout.

**Assumptions / Open Questions:**
- The broker local-path regrouping can be done as a pure module-tree change under `src/local/` with either direct call-site path updates or narrow reexports during the transition.
- The shared tunnel-open helpers in `port_forward/supervisor/open.rs` are still the exact seam reused by reconnect paths; implementation should confirm the final split point before moving code.
- `target/backend.rs` may remain as a tiny enum-dispatch module if moving it into `target/handle.rs` would create more churn than clarity; the goal is clearer ownership, not maximal flattening.
- For C++, “keep code clean” here means converge the specifically cited style inconsistencies, not launch a repo-wide naming rewrite.

---

### Task 1: Group broker-local implementation modules under a coherent `local/` namespace

**Intent:** Make the broker’s machine-local implementation path discoverable by grouping the current top-level local modules under one module tree.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/lib.rs`
- Likely create: `crates/remote-exec-broker/src/local/mod.rs`
- Likely move: `crates/remote-exec-broker/src/local_backend.rs`
- Likely move: `crates/remote-exec-broker/src/local_port_backend.rs`
- Likely move: `crates/remote-exec-broker/src/local_transfer.rs`
- Existing references: `crates/remote-exec-broker/src/startup.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/side.rs`
- Existing references: `crates/remote-exec-broker/src/tools/transfer/*`

**Notes / constraints:**
- Preserve current behavior and call-site semantics; this is a namespace cleanup, not a backend redesign.
- Prefer stable submodule names such as `local/backend.rs`, `local/port.rs`, and `local/transfer.rs` if they read better than carrying the old top-level filenames forward.
- Avoid transitional churn if a direct move and path update is simpler than temporary reexports.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: local transfer and local port-forward paths still behave identically after the namespace regrouping.

- [ ] Confirm the final `local/` submodule names and direct consumers before moving files
- [ ] Regroup the broker-local modules under a coherent `local/` namespace
- [ ] Update local-path call sites without widening visibility or changing behavior
- [ ] Run focused broker verification
- [ ] Commit

### Task 2: Split shared tunnel-open helpers out of `supervisor/open.rs`

**Intent:** Put the tunnel-open primitives in a module whose name reflects their cross-cutting responsibility instead of leaving them inside the open-a-new-forward orchestration file.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely create: `crates/remote-exec-broker/src/port_forward/supervisor/tunnel_open.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`

**Notes / constraints:**
- Keep the user-facing open-forward orchestration in `open.rs`; move only the shared tunnel/session-open primitives used by both open and reconnect flows.
- Do not change timeout behavior, forward generation handling, or open/reconnect wire flow while moving the helpers.
- Re-export only what the parent `supervisor` module actually needs; avoid creating a new miscellaneous module.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: forward open and reconnect behavior remains unchanged, while the tunnel-open ownership becomes easier to locate.

- [ ] Confirm the exact helper boundary between “open orchestration” and “shared tunnel opening”
- [ ] Move the shared tunnel-open primitives into a dedicated module with a clearer name
- [ ] Update reconnect/open call sites and module wiring
- [ ] Run focused forward-port verification
- [ ] Commit

### Task 3: Rename or relocate `exec/support.rs` to reflect its actual role

**Intent:** Replace the vague `support.rs` name with a name or placement that matches the exported workdir/path/sandbox/response helper surface.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/exec/mod.rs`
- Likely move: `crates/remote-exec-host/src/exec/support.rs`
- Existing references: `crates/remote-exec-host/src/exec/handlers.rs`
- Existing references: `crates/remote-exec-host/src/exec/session/*`

**Notes / constraints:**
- This is a naming/placement cleanup, not a semantic redesign of the helper APIs.
- Favor a concrete module name such as `policy.rs` or another name that matches the actual exports; implementation should pick the name that best fits the current contents after re-reading the callers.
- Preserve the current `pub use` surface from `exec/mod.rs` unless there is a very small internal-only cleanup that clearly improves readability.

**Verification:**
- Run: `cargo test -p remote-exec-host exec::`
- Expect: exec helper behavior and callers remain unchanged, with clearer module naming.

- [ ] Confirm the best destination/name for the current `support.rs` helper surface
- [ ] Move or rename the module and update `exec/mod.rs` exports
- [ ] Update internal call sites without broad API churn
- [ ] Run focused host exec verification
- [ ] Commit

### Task 4: Separate generic HTTP-serving glue from `tls.rs`

**Intent:** Make the daemon transport-serving module names match their responsibilities by moving non-TLS HTTP bind/serve helpers out of `tls.rs`.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon/src/tls.rs`
- Likely create: `crates/remote-exec-daemon/src/http_serve.rs` or `crates/remote-exec-daemon/src/server_transport.rs`
- Likely modify: `crates/remote-exec-daemon/src/lib.rs`
- Likely modify: `crates/remote-exec-daemon/src/server.rs`
- Existing references: `crates/remote-exec-daemon/src/test_support.rs`
- Existing references: `crates/remote-exec-daemon/src/tls_enabled.rs`

**Notes / constraints:**
- Leave TLS feature gating and TLS-specific validation in `tls.rs`.
- Move generic listener-binding and HTTP-serving helpers into a neutral transport-serving module only if that produces clearer ownership than the current layout.
- Preserve startup, shutdown, and feature-disabled error behavior exactly.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test health`
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Run: `cargo test -p remote-exec-daemon --lib tls`
- Expect: daemon serving and TLS feature-disabled behavior remain unchanged while the module naming becomes accurate.

- [ ] Confirm the final transport-serving module name and the exact functions to move
- [ ] Move generic HTTP/bind helpers out of `tls.rs` while keeping TLS validation/dispatch there
- [ ] Update daemon startup/server/test-support call sites
- [ ] Run focused daemon verification
- [ ] Commit

### Task 5: Apply bounded C++ consistency cleanup for constants and include guards

**Intent:** Keep the C++ side tidy by normalizing the specifically cited naming and header-guard inconsistencies without starting a repo-wide style churn pass.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/include/path_utils.h`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_filesystem.h`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_text_file.h`

**Notes / constraints:**
- Converge the cited constant declarations toward one local convention; prefer the convention already dominant in the touched area rather than inventing a new mixed rule.
- Convert the include-guard outliers to `#pragma once`, matching the prevailing production-header style in this codebase.
- Do not expand this into a full project-wide renaming or a new style-guide document in this pass unless a tiny README/comment change is unavoidable to explain a local choice.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the C++ daemon still builds and tests cleanly after the naming/header consistency cleanup.

- [ ] Confirm the local constant-naming convention to apply in the specifically cited files
- [ ] Normalize the cited constant names and update any direct local uses
- [ ] Convert the include-guard outliers to `#pragma once`
- [ ] Run focused C++ verification
- [ ] Commit

### Task 6: Close the section with a narrow `target/backend.rs` decision and final sweep

**Intent:** Resolve the small remaining naming/ownership question around `target/backend.rs` without reopening the larger backend abstraction debate, then run the final section-9 sweep.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/target/backend.rs`
- Likely modify: `crates/remote-exec-broker/src/target/handle.rs`
- Existing references: `crates/remote-exec-broker/src/startup.rs`

**Notes / constraints:**
- If keeping `target/backend.rs` as a tiny enum-dispatch module is clearer than flattening it, that is acceptable; the task is to make the ownership obvious, not to satisfy the audit literally.
- Do not introduce the larger trait-object redesign from section 6 in this pass.
- Use this task to review the whole section-9 diff for scope control and naming consistency.

**Verification:**
- Run: `cargo fmt --all --check`
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Run: `cargo test -p remote-exec-daemon --test health`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: final structure/naming cleanup compiles cleanly, passes focused tests, and stays within the approved scope.

- [ ] Make the narrow `target/backend.rs` ownership decision based on the actual final code shape
- [ ] Review the full section-9 diff for scope creep or stale-audit cargo culting
- [ ] Run the final section-9 verification sweep
- [ ] Commit
