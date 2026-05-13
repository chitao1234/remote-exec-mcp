# Round 5 Phase 4 Verified Ad Hoc Patterns Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Round 5 Phase 4 ad hoc patterns with focused Rust and C++ cleanups while preserving the current public RPC, config, and port-forward contracts.

**Requirements:**
- Cover the verified current-code follow-up for audit items `#20` through `#27`.
- Preserve all existing wire values, legacy aliases, and error-body compatibility for `RpcErrorCode`, transfer enums, and warning codes.
- Preserve exec RPC wire shape and validation semantics, including the requirement that running responses include `daemon_session_id` and completed responses omit it.
- Preserve transfer header names, parsing behavior, default values, and existing typed bad-request error messages.
- Preserve the current `[port_forward_limits]` TOML field surface even if the Rust implementation groups capacity and timeout concerns internally.
- Preserve port-tunnel wire behavior, retained-session resume semantics, and broker/daemon interoperability.
- Preserve the C++ daemon’s POSIX build, Windows XP-compatible build assumptions, and current HTTP/port-forward behavior. Do not introduce modern C++ helper patterns that would compromise that compatibility.

**Architecture:** Execute this phase as three medium-sized batches plus a final sweep. First, reduce proto-level ad hoc wire/parsing patterns by introducing local shared generation for string-wire enums, replacing the closure-based transfer-header parser with a typed input shape, and simplifying the exec response wire model without changing its JSON contract. Second, reduce host-side ad hoc patterns by adding internal frame-construction helpers, grouping port-forward capacity versus timeout concerns behind a stable config surface, and decomposing `tunnel_open_listen(...)` into smaller orchestration helpers. Third, add a small C++ consistency layer for HTTP 101 upgrade rendering and logging-message construction, then migrate the currently verified logging-specific `ostringstream` seams to it without widening into unrelated string renderers.

**Verification Strategy:** Verify each batch with the narrowest crate and integration tests that cover the touched seam, then finish with the full Rust quality gate plus the relevant C++ POSIX checks. Because Phase 4 spans Rust and C++, the final sweep should combine `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and at least one focused C++ `make` gate for the touched daemon code.

**Assumptions / Open Questions:**
- The safest implementation for `#20` is a local macro/helper inside `remote-exec-proto`, not a new third-party dependency such as `strum`.
- `RpcErrorCode` must continue to accept the legacy `"internal"` alias even if its wire-mapping code becomes generated.
- The safest implementation for `#21` likely replaces the callback parser with a small typed header collection or parsed-header input struct, because `remote-exec-proto` cannot depend directly on `reqwest` or `axum`.
- The safest implementation for `#22` is to simplify the wire model while preserving the current public `ExecResponse`, `ExecRunningResponse`, and `ExecCompletedResponse` usage in host and broker code.
- For `#23`, an internal host-local frame helper layer is safer than adding new public `Frame` constructors to `remote_exec_proto`.
- For `#26`, use internal grouping plus `serde(flatten)` or an equivalent compatibility approach if the field layout changes, so config examples and tests do not need a user-facing TOML migration.
- For `#24`, the verified logging problem is narrower than the raw `ostringstream` count suggests: several `ostringstream` uses are non-logging renderers and should stay out of scope unless execution proves the helper can absorb them mechanically.

**Planning-Time Verification Summary:**
- `#20`: still live. `crates/remote-exec-proto/src/transfer.rs`, `src/rpc/error.rs`, and `src/rpc/warning.rs` still hand-write enum-to-string mappings; no local macro or shared helper currently exists.
- `#21`: still live. `parse_transfer_export_metadata(...)` and `parse_transfer_import_metadata(...)` in `crates/remote-exec-proto/src/rpc/transfer.rs` still accept header lookups via closures, and broker/daemon adapters still feed them through callback wrappers.
- `#22`: still live. `crates/remote-exec-proto/src/rpc/exec.rs` still uses `ExecResponseWire` plus custom `Serialize`/`Deserialize` implementations to bridge `ExecResponse`.
- `#23`: still live, but narrower than the audit phrasing after earlier host refactors. Host port-forward production code still open-codes frame metadata encode/decode and frame assembly across `tunnel.rs`, `tcp.rs`, and `udp.rs`.
- `#24`: still live, but narrower than the raw count suggests. There are still 33 total `std::ostringstream` uses in `crates/remote-exec-daemon-cpp/src`, and a meaningful subset remain specifically for logging-message construction.
- `#25`: still live. `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp` still hand-concatenates the HTTP 101 upgrade response while `http_helpers.cpp` only provides `render_http_response(...)`.
- `#26`: still live. `HostPortForwardLimits` is still one flat struct that mixes capacity and timeout concerns, and its fields are used directly in broker/daemon config parsing and tests.
- `#27`: still live. `tunnel_open_listen(...)` in `crates/remote-exec-host/src/port_forward/tunnel.rs` still performs resume/create, mode-claim, attach, ready-send, and UDP reactivation in one function.

---

### Task 1: Proto Wire Mapping And Transfer Parsing Cleanup

**Intent:** Reduce the ad hoc enum-wire and transfer-header parsing patterns in `remote-exec-proto`, and simplify the exec response wire bridge without changing the public JSON contract.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/transfer.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/error.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/warning.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/transfer.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/exec.rs`
- Likely create: `crates/remote-exec-proto/src/[confirm helper filename for generated wire mappings]`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/codec.rs`
- Likely modify: `crates/remote-exec-daemon/src/transfer/codec.rs`
- Existing references: `crates/remote-exec-host/src/exec/handlers.rs`
- Existing references: `crates/remote-exec-host/src/exec/support.rs`
- Existing references: `crates/remote-exec-broker/src/tools/exec.rs`

**Notes / constraints:**
- Keep all existing wire values stable, including `RpcErrorCode::Internal` accepting both `"internal_error"` and the legacy `"internal"` alias.
- Keep transfer-header parse errors byte-for-byte compatible where tests or caller behavior already depend on them.
- Avoid adding a new workspace dependency just to generate string-wire mappings; keep the cleanup local and mechanical.
- Do not widen the exec response cleanup into a public tool or CLI behavior change; this task is about reducing proto-internal shape complexity while keeping all current callers working.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Expect: proto enum, transfer-header, and exec-response tests still pass with the simplified internals.
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Expect: daemon exec RPC behavior still passes against the simplified exec response wire model.
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: daemon transfer RPC parsing and export/import behavior still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Expect: broker exec behavior still passes end-to-end.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker transfer behavior still passes end-to-end.

- [ ] Confirm the exact live enum-mapping seams and choose a local helper or macro shape that can support both ordinary round trips and the `RpcErrorCode` legacy alias case
- [ ] Replace the closure-based transfer-header parser input with a typed proto-owned input shape and update broker/daemon adapters to populate it
- [ ] Simplify the exec response wire bridge while preserving current wire JSON and malformed-response validation behavior
- [ ] Run focused proto, daemon, and broker verification for exec and transfer paths
- [ ] Commit with real changes only

### Task 2: Host Port-Forward Frame And Config Pattern Cleanup

**Intent:** Reduce remaining ad hoc host-side frame construction and orchestration patterns without reopening the ownership changes already completed in Phase 3.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Likely modify: `crates/remote-exec-host/src/port_forward/udp.rs`
- Likely modify: `crates/remote-exec-host/src/config/mod.rs`
- Likely modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Likely modify: `crates/remote-exec-broker/src/config.rs`
- Likely create: `crates/remote-exec-host/src/port_forward/[confirm internal frame helper filename]`
- Existing references: `crates/remote-exec-host/src/port_forward/access.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/port_tunnel_tests.rs`
- Existing references: `crates/remote-exec-daemon/tests/port_forward_rpc.rs`
- Existing references: `crates/remote-exec-daemon/src/config/tests.rs`
- Existing references: `configs/*.example.toml`

**Notes / constraints:**
- Keep the frame-helper layer internal to the host port-forward implementation; do not add new public `Frame` constructors unless execution proves that to be the only coherent option.
- Preserve all current tunnel error codes, limit behavior, and retained-session resume semantics.
- If `HostPortForwardLimits` is internally regrouped, preserve the current `[port_forward_limits]` field names and defaults so existing configs and tests remain valid without a migration.
- Keep the `tunnel_open_listen(...)` split focused on orchestration readability. Do not reopen the already-completed listen-session ownership refactor from Phase 3.

**Verification:**
- Run: `cargo test -p remote-exec-host port_forward::port_tunnel_tests`
- Expect: host port-tunnel behavior still passes after the helper extraction and function split.
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: daemon port-forward RPC behavior remains unchanged.
- Run: `cargo test -p remote-exec-daemon --lib config::tests`
- Expect: daemon config parsing still accepts the current `port_forward_limits` surface and validates limits correctly.
- Run: `cargo test -p remote-exec-broker --lib config::tests`
- Expect: broker config parsing still accepts the embedded host limits shape unchanged.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: broker forwarding behavior still passes against the Rust host/daemon path.

- [ ] Confirm the remaining frame-construction duplication across `tunnel.rs`, `tcp.rs`, and `udp.rs`, then introduce a focused internal helper layer for the repeated frame/meta assembly
- [ ] Group host port-forward capacity versus timeout concerns behind a clearer internal config boundary while preserving the current external TOML field surface
- [ ] Split `tunnel_open_listen(...)` into smaller helpers for session acquisition, mode/open sequencing, and ready/reattivation follow-through
- [ ] Run focused host, daemon, and broker verification for forwarding and config parsing
- [ ] Commit with real changes only

### Task 3: C++ Upgrade And Logging Consistency

**Intent:** Remove the remaining ad hoc C++ HTTP upgrade response rendering and logging-message construction seams without widening into unrelated string-rendering internals.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/include/http_helpers.h`
- Likely modify: `crates/remote-exec-daemon-cpp/src/http_helpers.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_route_exec.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/session_store.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Likely modify: other C++ files that currently use `std::ostringstream` specifically for logging-message assembly
- Existing references: `crates/remote-exec-daemon-cpp/mk/posix.mk`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Keep the helper design compatible with the daemon’s POSIX and Windows XP-oriented C++11 build constraints; do not assume modern variadic-template or formatting-library availability.
- Keep this task scoped to logging-message construction and the HTTP 101 upgrade response helper. Do not rewrite unrelated `ostringstream` uses that serialize HTTP bodies, patch text, or other non-logging payloads unless the migration is purely mechanical and obviously beneficial.
- Preserve the exact upgrade headers that broker tests and runtime interoperability depend on, especially `Connection`, `Upgrade`, and request-id propagation.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the POSIX C++ daemon build and test suite still pass after the helper introduction.
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: the C++ server streaming path still passes after touching HTTP helpers.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Expect: broker forwarding still interoperates with the real C++ daemon after the upgrade-response change.

- [ ] Distinguish verified logging-message `ostringstream` sites from unrelated string-rendering helpers and confirm the exact migration set
- [ ] Add a C++11-compatible helper for the raw HTTP upgrade response and a small helper for repetitive logging-message assembly
- [ ] Migrate the verified logging-message seams and the port-tunnel upgrade path to those helpers without changing behavior
- [ ] Run focused C++ and broker verification
- [ ] Commit with real changes only

### Task 4: Final Phase 4 Confirmatory Sweep

**Intent:** Reconfirm which Phase 4 seams were live, document the narrowed interpretations where needed, and finish with the cross-language quality gates.

**Relevant files/components:**
- Likely inspect: `docs/CODE_AUDIT_ROUND5.md`
- Likely inspect: the Rust and C++ files touched by Tasks 1 through 3
- Likely inspect: `configs/*.example.toml`

**Notes / constraints:**
- Keep the final summary explicit where the verified scope was narrower than the original audit phrasing, especially for `#23` and `#24`.
- Re-run targeted searches so the completion notes distinguish “implemented”, “already narrowed by earlier phases”, and “intentionally scoped more tightly than the audit suggestion”.
- If any batch proves safer with a smaller helper-only cleanup than the original audit recommendation, document that precisely instead of overstating the result.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the Rust workspace passes end-to-end after the full Phase 4 bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting stays clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions after the Rust cleanups.
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: touched C++ code still passes the POSIX build/test gate.

- [ ] Re-run targeted searches for items `#20` through `#27` and confirm which claims were exact, narrower than written, or required compatibility-scoped implementations
- [ ] Run the full Rust quality gate and the relevant C++ POSIX gate for the combined cross-language refactor
- [ ] Summarize the narrowed interpretation for `#23` and `#24` if execution keeps those fixes intentionally tighter than the original audit text
- [ ] Commit any sweep-only adjustments if needed; otherwise do not create an empty commit
