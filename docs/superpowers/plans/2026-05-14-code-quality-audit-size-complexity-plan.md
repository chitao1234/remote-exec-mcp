# Code Quality Audit Size And Complexity Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the worthwhile live `2.*` complexity items from `docs/code-quality-audit.md` with narrow, owner-local refactors that improve clarity or correctness without reopening broad structural redesign.

**Requirements:**
- Keep the implementation scope narrow: `2.9` and `2.3` are in scope, `2.2` is optional only if it still has a clear local helper boundary during execution, and `2.1` is in scope only as a small opportunistic follow-on if execution is already touching `open.rs`.
- Preserve public tool behavior, broker-daemon wire format, port-forward semantics, exec session semantics, config file format, and RPC error codes.
- Do not turn this pass into a broad “group big structs” refactor. Flat serde/config structs and public wire-contract enums stay flat unless a directly related task proves otherwise.
- Keep changes owner-local to the responsible modules. Prefer helper extraction or smaller sequencing cleanup over new abstraction layers.
- Do not modify `docs/code-quality-audit.md`; it remains read-only input.
- Continue the established execution style: medium-sized tasks, focused verification after each task, no empty commits.

**Architecture:** Treat the live `2.*` findings as one correctness cleanup, one clarity refactor, one optional local follow-up, and a final sweep. First, tighten the host port-tunnel shutdown path so `close_tunnel_runtime` reads and consumes `tunnel.active` under a single lock acquisition. Second, split `exec_write_local` at the lock-and-precondition seam so request validation and session orchestration are easier to reason about while preserving all error mapping and logging. Third, only if the helper boundaries remain obviously local and behavior-preserving, peel small pieces out of `tunnel_tcp_accept_loop` and, optionally, `build_opened_forward`; do not force decomposition just because a function crosses an arbitrary line count.

**Verification Strategy:** Run focused tests after each task in the touched crate areas, then finish with the Rust quality gate on the current HEAD: `cargo test --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

**Assumptions / Open Questions:**
- `2.9` should be implemented even if it is a small change, because it removes a genuine interleaving window rather than merely shortening code.
- `2.3` should favor extraction of the lock-and-validate precondition phase, not a wider redesign of the exec write path.
- `2.2` should proceed only if execution confirms a helper split that does not hide the accept-loop control flow or worsen ownership visibility around permits, stream registration, and spawned tasks.
- `2.1` should remain optional and small. `open.rs` has already been partially decomposed, so only residual orchestration seams that clearly improve readability without scattering the forward-open flow are in scope.
- `2.4`, `2.5`, `2.6`, and `2.8` are intentionally deferred as low-value structural churn for now; `2.7` is intentionally rejected because it would reshape public wire-contract surface.

**Planning-Time Verification Summary:**
- `2.1`: partially valid and narrowed. [open.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/supervisor/open.rs:207) is still large, but `open_protocol_forward`, `open_listen_session_for_forward`, `open_connect_tunnel_for_forward`, and `wait_for_listener_ready` already exist, so the “god function” claim is partly stale. Only a small residual extraction is in scope.
- `2.2`: valid and optional. [tcp.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward/tcp.rs:172) still mixes accept, pressure handling, stream registration, accept-frame emission, and read-loop spawn.
- `2.3`: valid and in scope. [handlers.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/exec/handlers.rs:75) still mixes session lock acquisition, PTY validation, stdin policy, write/poll orchestration, and response shaping.
- `2.4`: factually true but deferred. [mod.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon/src/config/mod.rs:29) still has 18 fields, but it is the serde-loaded daemon config surface and a conversion target from `EmbeddedHostConfig`, so grouping would create config churn with weak payoff.
- `2.5`: deferred and partly stale. [state.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/state.rs:41) currently has 12 fields, not 13, and it already acts as the top-level runtime aggregate owner.
- `2.6`: partially valid but deferred. [session.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward/session.rs:21) has 10 fields, but the mutex-wrapped buckets correspond to distinct concurrent resources, so regrouping is not clearly beneficial.
- `2.7`: explicitly out of scope. [error.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-proto/src/rpc/error.rs:12) currently has 46 variants, not 38, and is public wire-contract surface. Nested enums would be broad contract churn, not a local complexity cleanup.
- `2.8`: factually true but deferred. [tunnel.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward/tunnel.rs:118) has nested control flow, but it remains compact and closely tied to frame/error handling.
- `2.9`: valid and in scope. [tunnel.rs](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-host/src/port_forward/tunnel.rs:451) currently acquires `tunnel.active` once to check for `Listen` and again to `take()` the `Connect` runtime, leaving a real interleaving window.

---

### Task 1: Tighten Tunnel Shutdown State Consumption

**Intent:** Remove the double-lock shutdown pattern in `close_tunnel_runtime` so tunnel role inspection and active-runtime consumption happen under one lock acquisition.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/session.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/active.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`

**Notes / constraints:**
- Cover audit item `2.9`.
- Preserve existing `Listen` versus `Connect` shutdown behavior, including when listen-side shutdown delegates to `close_attached_session`.
- Do not introduce lock-order inversions with the follow-on session/stream cleanup paths.
- Prefer a small local rewrite of the `active` extraction logic over new helper types unless a helper clearly improves readability.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: tunnel open/close and listener release behavior still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker-facing forward lifecycle behavior still passes against the Rust daemon path.

- [ ] Reconfirm the exact `active` state transitions and shutdown call graph in the current port-forward tunnel path
- [ ] Rewrite `close_tunnel_runtime` so role inspection and `active.take()` happen under one lock acquisition
- [ ] Add or update focused tests only if the refactor changes an observable shutdown edge case
- [ ] Run focused daemon and broker port-forward verification
- [ ] Commit with real changes only

### Task 2: Split `exec_write_local` At The Session-Validation Seam

**Intent:** Extract the lock-and-precondition phase from `exec_write_local` so session acquisition, PTY validation, and stdin-policy checks are separated from write/poll/response orchestration.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/exec/handlers.rs`
- Existing references: `crates/remote-exec-host/src/exec/support.rs`
- Existing references: `crates/remote-exec-host/src/exec/store.rs`
- Existing references: `crates/remote-exec-daemon/tests/exec_rpc/`

**Notes / constraints:**
- Cover audit item `2.3`.
- Preserve all current error codes, status codes, error wording, and logging fields.
- Keep the extracted helper private to the exec handlers module unless execution proves an immediately adjacent shared caller.
- The goal is not to change behavior; it is to isolate the “obtain writable session and validate request preconditions” phase from the rest of the handler.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: exec write, PTY resize, stdin-closed, timeout, and unrelated-session behavior still passes.

- [ ] Reconfirm the natural helper seam in `exec_write_local` and the exact error mapping obligations
- [ ] Extract the session-lock and request-precondition phase into the smallest useful private helper
- [ ] Keep write/poll/completed-vs-running response behavior unchanged
- [ ] Run focused exec RPC verification
- [ ] Commit with real changes only

### Task 3: Optional Local Complexity Follow-Up

**Intent:** Apply one additional small complexity cleanup only if execution confirms a clear, behavior-preserving boundary in the remaining live `2.*` functions.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Optionally modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/session.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`

**Notes / constraints:**
- Primary candidate: `2.2` in `tunnel_tcp_accept_loop`.
- Secondary candidate, only if already in the file and still obviously local: a small residual extraction from `2.1` in `build_opened_forward`.
- Do not force a task here if the best available extraction merely moves code around without clarifying ownership or sequencing.
- If neither candidate stays clean during execution, intentionally skip this task and record that outcome during the final sweep.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: host port-forward tunnel behavior still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker public forwarding behavior still passes.

- [ ] Reconfirm whether `tunnel_tcp_accept_loop` still has a clean helper boundary after Task 1
- [ ] Implement one small local extraction only if it clearly reduces orchestration noise without obscuring ownership
- [ ] Touch `build_opened_forward` only if execution is already in `open.rs` and a residual helper falls out naturally
- [ ] Run the focused port-forward verification for any touched path
- [ ] Commit with real changes only, or explicitly skip this task if no worthwhile extraction remains

### Task 4: Final `2.*` Sweep And Rust Quality Gate

**Intent:** Confirm that the scoped `2.*` pass fixed the intended items, left the deferred items intentionally deferred, and did not introduce regressions.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely inspect: `crates/remote-exec-host/src/exec/handlers.rs`
- Optionally inspect: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Optionally inspect: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`

**Notes / constraints:**
- Keep this confirmatory. Do not widen into Section `3.*` or `4.*` of the audit.
- Reconfirm that `2.7` stays explicitly out of scope and `2.4` through `2.8` remain deferred unless one was already intentionally handled under Task 3.
- Use final notes to distinguish “fixed”, “intentionally deferred”, and “rejected as not worth changing”.

**Verification:**
- Run: `cargo test --workspace`
- Expect: full Rust workspace passes on the final HEAD.
- Run: `cargo fmt --all --check`
- Expect: formatting is clean on the final HEAD.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions on the final HEAD.

- [ ] Re-run targeted searches or diffs for the `2.*` seams addressed in this plan
- [ ] Run the full Rust quality gate on the final HEAD
- [ ] Summarize which `2.*` items were fixed, deferred, or intentionally rejected
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
