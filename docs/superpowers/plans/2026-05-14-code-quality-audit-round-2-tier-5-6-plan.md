# Code Quality Audit Round 2 Tier 5 and Tier 6 Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Tier 5 and Tier 6 issues from `docs/code-quality-audit-round-2.md` without changing transfer wire values, broker public tool behavior, or shared Rust/C++ transport contracts.

**Requirements:**
- Keep transfer HTTP header names and wire values unchanged. `overwrite` must still accept and emit `fail`, `merge`, and `replace`.
- Narrow Tier 6 to the seams that are still live in the current tree. Do not churn the audit items that are no longer single-use or no longer over-abstracted in practice.
- Preserve broker public tool arguments and result shapes, daemon RPC behavior, and the broker/C++ daemon transfer contract.
- Keep the C++ daemon within the repository's C++11 and XP-compatible toolchain expectations. Do not introduce newer language requirements.
- Treat the `NULL` cleanup as a production C++ sweep only. Do not expand it into `third_party/` or test-only files as part of this plan.
- Keep commits medium-sized. Mechanical cleanup tasks should stay behavior-neutral.

**Architecture:** Use a typed C++ overwrite enum immediately after HTTP header parsing, then thread that enum through transfer metadata and the transfer import filesystem helpers so overwrite validation happens once at the boundary. Collapse the C++ import overload ladder down to two real entry points that rely on C++11 default arguments instead of six forwarding wrappers. On the Rust side, inline the genuinely single-use broker exec helpers and replace the enum-dispatch TCP read-loop target with a private trait over the existing connect and listen contexts, while explicitly leaving already-reused helpers in place.

**Verification Strategy:** Run focused C++ transfer and Rust exec/port-forward tests after each task. Finish with the quality gates required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `make -C crates/remote-exec-daemon-cpp check-posix`.

**Assumptions / Open Questions:**
- The C++ transfer layer can add a local typed overwrite enum and parser helpers without changing any broker-visible or user-visible wire contract.
- The production `NULL` cleanup may be committed in one or two mechanical commits during execution if the touched-file set gets too noisy, but it remains one planned task.
- For the TCP read loop, confirm during execution whether a private trait in `tcp.rs` is sufficient or whether the shared methods belong on the existing context types without introducing any new public surface.

**Planning-Time Verification Summary:**
- `5.1`: valid and in scope. The C++ daemon still carries `overwrite_mode` as raw strings through `transfer_http_codec`, `transfer_ops.h`, `transfer_ops_internal.h`, `transfer_ops_fs.cpp`, and `transfer_ops_import.cpp`.
- `5.2`: valid and in scope. Excluding `third_party` and tests, production C++ still contains a broad `NULL` footprint across port-tunnel, runtime, process, path, and socket code.
- `6.1`: valid and in scope. `ToolOperationError` in `crates/remote-exec-broker/src/tools/exec.rs` is constructed only for `write_stdin`.
- `6.2`: valid and in scope. `TcpReadLoopTarget` in `crates/remote-exec-host/src/port_forward/tcp.rs` is still a hand-rolled two-variant dispatch enum with duplicated method forwarding.
- `6.3`: valid and in scope. `import_path` and `import_path_from_reader` in `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp` still form a six-overload forwarding cascade.
- Out of scope by verification: `running_session_response`, `session_limit_warnings`, `invalid_enum_header`, and `apply_daemon_client_timeouts` are now reused helpers. `format_command_text` and `format_poll_text` remain low-value churn rather than a meaningful simplification target.

---

### Task 1: Type The C++ Transfer Overwrite Path And Collapse Import Overloads

**Intent:** Cover Tier `5.1` and Tier `6.3` together by turning C++ overwrite handling into a typed internal API and removing the six-overload forwarding ladder from the import path.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/transfer_ops.h`
- Likely modify: `crates/remote-exec-daemon-cpp/include/transfer_http_codec.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Likely inspect or modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`

**Notes / constraints:**
- Keep the wire contract unchanged. Header validation should still reject unsupported overwrite values at the HTTP boundary with the same class of bad-request error.
- Prefer a single typed `TransferOverwrite` enum and small conversion helpers over repeated string comparisons in internal code.
- Internal C++ callers and tests should no longer need to pass raw overwrite strings once they are past the HTTP boundary.
- Replace the six implementation overloads with two real entry points plus default arguments in the public declaration surface. Do not leave a forwarding ladder in place under a different spelling.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
- Expect: transfer import/export behavior remains unchanged while typed overwrite handling covers the same file, directory, and multiple-source import cases.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: HTTP transfer header parsing and route glue still accept valid metadata and reject invalid metadata correctly.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker-visible transfer behavior stays aligned with the daemon contract.

- [ ] Reconfirm the live overwrite parsing and import call graph, including every place that still accepts raw overwrite strings internally
- [ ] Introduce the typed overwrite enum and conversion helpers at the C++ HTTP boundary and internal transfer API seam
- [ ] Collapse the `import_path` and `import_path_from_reader` overload cascade into two real functions with C++11 default arguments
- [ ] Update or add C++ transfer tests so typed overwrite handling and the slimmer import surface are exercised directly
- [ ] Run the focused transfer verification
- [ ] Commit with real changes only

### Task 2: Remove The Still-Live Rust Single-Use Helper Noise

**Intent:** Cover the live Rust-side Tier 6 items by removing the genuinely single-use broker exec helpers and replacing the TCP read-loop dispatch enum with a clearer private boundary.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/tools/exec.rs`
- Likely inspect: `crates/remote-exec-broker/src/tools/exec_format.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely inspect or modify: `crates/remote-exec-host/src/port_forward/active.rs`
- Likely inspect: `crates/remote-exec-broker/tests/mcp_exec/intercept.rs`
- Existing coverage: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`

**Notes / constraints:**
- In scope: `ToolOperationError`, `exec_start_request`, `forward_exec_start`, `apply_patch_warning`, and `TcpReadLoopTarget`.
- Out of scope: `running_session_response`, `session_limit_warnings`, `invalid_enum_header`, `apply_daemon_client_timeouts`, and the `exec_format` wrappers unless execution proves one of them becomes directly coupled to the in-scope cleanup.
- Preserve all current broker output formatting, warning text, request validation, and TCP tunnel behavior.
- If the TCP read-loop cleanup uses a private trait, keep it local to the port-forward internals rather than adding a broader abstraction layer.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec and write-stdin behavior, including intercepted apply-patch warnings, remain unchanged.
- Run: `cargo test -p remote-exec-host port_forward::port_tunnel_tests -- --nocapture`
- Expect: both listen-side and connect-side TCP tunnel behavior remain correct after the read-loop boundary change.

- [ ] Reconfirm the live call counts for the audited Rust helpers so only the genuinely single-use items are removed
- [ ] Inline or collapse the single-use broker exec helpers without changing tool behavior or output text
- [ ] Replace `TcpReadLoopTarget` with a clearer private connect/listen read-loop boundary
- [ ] Add or update focused tests if the helper removal changes the seam enough to merit direct coverage
- [ ] Run the focused Rust verification
- [ ] Commit with real changes only

### Task 3: Sweep Production C++ Pointer Nullability To `nullptr`

**Intent:** Cover Tier `5.2` with a production-only mechanical cleanup that removes remaining `NULL` usage from C++11 code without changing behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/scoped_file.h`
- Likely modify: `crates/remote-exec-daemon-cpp/include/win32_scoped.h`
- Likely modify: `crates/remote-exec-daemon-cpp/include/win32_thread.h`
- Likely modify: the touched production files under `crates/remote-exec-daemon-cpp/src/`, with the heaviest current clusters in `port_tunnel_session.cpp`, `server_runtime.cpp`, `port_tunnel_error.cpp`, `process_session_win32.cpp`, `path_compare.cpp`, `shell_policy.cpp`, `session_pump.cpp`, `port_tunnel.cpp`, and `port_forward_socket_ops.cpp`
- Existing integration coverage: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Do not touch `crates/remote-exec-daemon-cpp/third_party/` or test-only files in this task.
- Keep the sweep mechanical. Replace pointer-context `NULL` with `nullptr`; do not fold in unrelated cleanup.
- Where an API truly wants an integer sentinel rather than a pointer null value, confirm that during execution instead of blindly replacing it.
- If the diff becomes too large for one reviewable commit, split the sweep by module cluster while keeping the task behaviorally unified.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: the real C++ daemon still supports broker-driven TCP and UDP forwarding and listener cleanup behavior.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the production C++ daemon still compiles and passes its POSIX test suite after the mechanical nullability cleanup.

- [ ] Reconfirm the production-only `NULL` footprint and decide whether one or two mechanical commits best preserves reviewability
- [ ] Replace pointer-context `NULL` usage with `nullptr` across the targeted production C++ files
- [ ] Keep any non-pointer sentinel cases explicit if execution uncovers them
- [ ] Run the focused C++ verification
- [ ] Commit with real changes only

### Task 4: Final Tier 5 and Tier 6 Sweep And Full Quality Gate

**Intent:** Reconfirm which Tier 5 and Tier 6 items were actually fixed, which stale audit claims were intentionally left alone, and finish with the full cross-language quality gates.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit-round-2.md`
- Likely inspect: the touched Rust and C++ files from Tasks 1 through 3

**Notes / constraints:**
- Keep the sweep limited to the verified Tier 5 and Tier 6 items from Round 2. Do not expand into Tier 7+ work during this pass.
- Explicitly preserve the decision to skip the stale Tier 6 claims unless execution proves one became newly relevant.
- Do not create an empty sweep commit. Only commit here if the verification pass forces a real follow-up change.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the full Rust workspace passes end-to-end after the Tier 5/6 bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no new lint regressions are introduced.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the shared POSIX C++ quality gate remains green after all Tier 5/6 work.

- [ ] Re-run the Tier 5 and Tier 6 verification queries and confirm the final code shape is intentional for each audited seam
- [ ] Run the required Rust workspace quality gate
- [ ] Run the required C++ POSIX quality gate
- [ ] Summarize which Tier 5/6 items were fixed and which stale audit claims were intentionally left unchanged
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
