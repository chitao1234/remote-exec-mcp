# Code Quality Audit Dedup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live dedup items from Section 1 of `docs/code-quality-audit.md` using medium-sized, owner-local cleanup batches that preserve current broker, daemon, and test behavior.

**Requirements:**
- Cover every dedup claim in `docs/code-quality-audit.md` with an explicit planning-time disposition: in scope, narrowed, or deferred.
- Fix only the seams that are still real in current code; do not force abstractions for claims that have become stale or too weak to justify new structure.
- Preserve broker public behavior, daemon RPC semantics, port-forward recovery behavior, transfer behavior, and current test helper entry points unless a rename is required by the implementation and updated everywhere in the same task.
- Keep C++ changes compatible with the current C++11 codebase and existing POSIX and Windows XP-capable toolchain expectations.
- Continue the established execution style: medium-sized tasks, focused verification after each task, and no empty commits.
- Do not rewrite `docs/code-quality-audit.md`; it is input to this plan, not a live contract to edit.

**Architecture:** Treat the dedup work as three implementation batches plus a final sweep. First, collapse the exact host-owned duplication in `remote-exec-host` with private helpers or local macros at existing module boundaries. Second, clean up the low-risk broker-owned repetition in dispatch, reconnect bookkeeping, shared config validation, and transfer logging without widening the public API surface. Third, consolidate test-support duplication across Rust and C++ by introducing shared helper seams while preserving the existing top-level spawn helper API that integration tests already use.

**Verification Strategy:** Run focused verification after each task using the relevant Rust integration suites and C++ host-test targets for the touched areas, then finish with the cross-cutting quality gate required by `AGENTS.md`: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `make -C crates/remote-exec-daemon-cpp check-posix`.

**Assumptions / Open Questions:**
- Audit item `1.7` is no longer a meaningful core-logic duplication seam because `normalize_configured_workdir(...)` already owns the normalization logic; only thin wrapper methods remain, so this plan defers that item unless a cleanup falls out naturally from `1.6`.
- Audit item `1.13` is directionally correct but currently a weak abstraction target; this plan defers it unless a shared helper emerges naturally during host-side execution without making the polling flow harder to read.
- Audit item `1.9` stays in scope only as a narrowed shared polling helper for test support; the concrete probe bodies and daemon-thread-finished checks should remain explicit at the call sites.
- Audit item `1.11` should preserve the existing public spawn helper function names and factor only the internal fixture/config assembly path unless implementation proves a narrower API change is clearly safer.

**Planning-Time Verification Summary:**
- `1.1`: valid and in scope. `TcpReadLoopTarget` still duplicates pure `Connect` and `Listen` dispatch, and `tunnel_tcp_data` plus `tunnel_tcp_eof` still repeat the same branch structure.
- `1.2`: valid and in scope. `TransferError` and `ImageError` remain structurally duplicated in `crates/remote-exec-host/src/error.rs`.
- `1.3`: valid and in scope. `new_session` and `new_session_for_test` still differ only by the `CancellationToken` source.
- `1.4`: valid and in scope. `TargetHandle` still has six identical backend dispatch methods.
- `1.5`: valid and in scope. `mark_reconnecting` and `mark_connect_reopening_after_listen_recovery` still share most of their bookkeeping logic.
- `1.6`: valid and in scope. `validate_existing_directory` remains duplicated between broker and host config code.
- `1.7`: partially valid and deferred. The duplicated core normalization logic is already gone; only wrapper-level duplication remains.
- `1.8`: valid and in scope. `toml_string` is still copy-pasted between broker and daemon test support.
- `1.9`: partially valid and narrowed. The broker and daemon readiness loops still share the same poll-until-ready structure, but their probes and failure conditions are not identical.
- `1.10`: valid and in scope. The C++ `write_text_file` helpers remain duplicated across two tests.
- `1.11`: valid and in scope. `crates/remote-exec-broker/tests/support/spawners.rs` still contains a large set of near-duplicate fixture constructors that differ only by a small number of config knobs.
- `1.12`: partially valid and narrowed. `daemon_client.rs` already has some shared transport/error helpers, but the transfer export/import paths still repeat field-specific logging and decode wiring.
- `1.13`: partially valid and deferred. The host exec loops are structurally similar, but the clean helper boundary is still questionable.
- `1.14`: valid and in scope. `require_protocol`, `require_listen_session`, `require_connect_tunnel`, and `require_bind_target` remain repetitive in `active.rs`.

---

### Task 1: Collapse Exact Host-Owned Dedup Seams

**Intent:** Remove the still-exact duplication inside `remote-exec-host`, especially the port-forward and host-domain error seams, without introducing broader public abstractions.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/active.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Likely modify: `crates/remote-exec-host/src/error.rs`

**Notes / constraints:**
- Cover audit items `1.1`, `1.2`, `1.3`, and `1.14`.
- Prefer private helpers, small local macros, or constructor extraction over a new trait hierarchy unless the code already wants that shape.
- Preserve current error codes, message wording, tunnel generation behavior, and `Connect` versus `Listen` semantics.
- Keep the test-only cancellation-token injection path explicit even if the constructor becomes shared.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: host-backed port-forward RPC behavior still passes.
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: host transfer error handling still passes after the shared error-type cleanup.
- Run: `cargo test -p remote-exec-daemon --test image_rpc`
- Expect: host image error handling still passes after the shared error-type cleanup.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker public forwarding behavior still passes against the Rust daemon path.

- [ ] Reconfirm the exact duplicated shapes for `1.1`, `1.2`, `1.3`, and `1.14` at the current code locations
- [ ] Extract the smallest host-local helpers that collapse the duplication without changing public behavior
- [ ] Add or adjust focused tests only where helper extraction needs direct coverage
- [ ] Run the focused daemon and broker forwarding verification for the touched seams
- [ ] Commit with real changes only

### Task 2: Deduplicate Low-Risk Broker-Owned Rust Helpers

**Intent:** Collapse the broker-side repetition that is still justified to clean up now, while keeping the cleanup local to the current owner modules.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/target/handle.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/store.rs`
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely modify: `crates/remote-exec-broker/src/daemon_client.rs`
- Likely modify: `crates/remote-exec-host/src/config/mod.rs`

**Notes / constraints:**
- Cover audit items `1.4`, `1.5`, `1.6`, and the narrowed current-code follow-up for `1.12`.
- Do not widen the public `TargetHandle` or daemon-client surface just to remove a few repeated method bodies.
- For `1.6`, prefer exposing or reusing the existing host-side directory validator rather than creating a third copy.
- For `1.12`, keep the difference between generic RPC strict decoding and transfer-path lenient decoding explicit in the helper boundary.
- Do not force a separate task for `1.7`; only remove wrapper-level workdir normalization duplication if it naturally becomes cleaner while touching `1.6`.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker transfer behavior still passes, including import/export error handling.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: broker image/view-image and patch-adjacent asset coverage still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker port-forward store behavior still passes through the public test surface.
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Expect: CLI-facing broker behavior still passes after config/helper cleanup.

- [ ] Reconfirm the exact broker-owned duplication and the narrower live scope for `1.12`
- [ ] Extract or reuse the smallest shared helpers inside `TargetHandle`, `store.rs`, `config.rs`, and `daemon_client.rs`
- [ ] Keep decode-policy, error wording, and reconnect phase behavior stable while deduplicating
- [ ] Run the focused broker verification for transfer, assets, forwarding, and CLI coverage
- [ ] Commit with real changes only

### Task 3: Consolidate Rust Test Support And C++ Test Helpers

**Intent:** Reduce duplicated test-support setup without shrinking the useful top-level helper surface that the existing tests already depend on.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/tests/support/spawners.rs`
- Likely modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Likely modify: `crates/remote-exec-daemon/tests/support/spawn.rs`
- Likely modify: `crates/remote-exec-daemon/tests/support/mod.rs`
- Likely create: `tests/support/[shared helper file for TOML and readiness polling]`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes_shared.cpp`

**Notes / constraints:**
- Cover audit items `1.8`, `1.9`, `1.10`, and `1.11`.
- Preserve current public spawn helper names in `spawners.rs`; the cleanup target is the internal config assembly path, not a sweeping rename of test call sites.
- For `1.9`, share the polling skeleton only; keep the HTTP health probe, MCP initialize probe, and daemon thread-finished checks explicit at each call site.
- For `1.10`, prefer one shared C++ test helper with local include reuse rather than copy-pasting another file-level utility.
- Reuse the existing `tests/support/` shared-module pattern already used for `transfer_archive.rs`.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_http`
- Expect: broker HTTP readiness and streamable HTTP test setup still passes.
- Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- Expect: broker multi-target fixture assembly still passes after internal spawner cleanup.
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: daemon readiness support still passes after the shared polling helper extraction.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
- Expect: the session-store C++ test still passes with the shared text-file helper.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-routes`
- Expect: the server-routes C++ test still passes with the shared text-file helper.

- [ ] Reconfirm the shared-helper opportunities in Rust test support and the C++ test files
- [ ] Introduce the smallest shared `tests/support` helper seam for TOML quoting and readiness polling
- [ ] Refactor broker spawner internals around a common builder or fixture-construction path while preserving current public helper entry points
- [ ] Apply the shared C++ text-file helper in both duplicated test files
- [ ] Run the focused Rust and C++ test verification for the touched support paths
- [ ] Commit with real changes only

### Task 4: Final Dedup Sweep And Quality Gate

**Intent:** Confirm that the planned dedup batch actually removed or intentionally narrowed the targeted seams and that the combined Rust and C++ tree still passes the required quality gate.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: the code paths touched by Tasks 1 through 3

**Notes / constraints:**
- Use targeted search or diff checks to confirm the audited duplication was removed or deliberately reduced to one owner.
- Keep this task as a confirmatory sweep; do not widen into the non-dedup sections of `docs/code-quality-audit.md`.
- If any narrowed or deferred item remains intentionally unresolved, document that outcome in the implementation notes rather than forcing a risky abstraction.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the full Rust workspace passes after the dedup bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting remains clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the touched C++ code and host tests still pass in the POSIX build path.

- [ ] Re-run searches for the audited duplicate seams and confirm the final surviving code shape is intentional
- [ ] Run the required Rust workspace quality gate
- [ ] Run the relevant C++ POSIX quality gate
- [ ] Summarize which claims were fixed, narrowed, or intentionally deferred
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
