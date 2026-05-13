# Round 5 Phase 5 Verified Structural Holdouts Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live Round 5 Phase 5 structural holdouts with focused Rust refactors that improve internal shape without changing the current broker, sandbox, or PKI user-facing behavior.

**Requirements:**
- Cover the verified current-code follow-up for audit items `#28` through `#30`.
- Preserve the current public `forward_ports` behavior, reconnect semantics, broker-owned `forward_id` handling, and existing broker integration tests.
- Preserve the current `remote_exec_proto::sandbox` public import surface, canonicalization behavior, authorization outcomes, and existing sandbox error text.
- Preserve the current `certs-manifest.json` serialized field names and `render_config_snippets(...)` output while reducing PKI path duplication.
- Keep this phase Rust-only unless execution proves a user-facing documentation update is required; do not widen into unrelated broker, daemon, or C++ behavior changes.

**Architecture:** Execute this phase as three medium-sized Rust batches plus a final sweep. First, split broker port-forward forward identity from mutable runtime handles so task and reconnect code stop carrying one mixed runtime bag. Second, split `remote-exec-proto` sandbox logic into a small module tree that keeps the current root-level exports stable while isolating types, authorization, and reusable path helpers. Third, compose `DaemonManifestEntry` around `KeyPairPaths` internally while preserving the serialized manifest shape through `serde(flatten)` or an equivalent compatibility-safe approach.

**Verification Strategy:** Verify each batch with the narrowest crate and integration tests that cover the touched seam, then finish with the Rust workspace quality gate. Because the verified Phase 5 items are Rust-only, the final sweep should focus on `cargo test --workspace`, `cargo fmt --all --check`, and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

**Assumptions / Open Questions:**
- The safest implementation for `#28` is likely a new internal `ForwardIdentity` type plus a slimmer `ForwardRuntime`, rather than a broader supervisor architecture rewrite.
- The safest implementation for `#29` is likely a `sandbox/` module tree with stable `pub use` re-exports from `mod.rs`, so downstream crates keep using `remote_exec_proto::sandbox::...`.
- The safest implementation for `#30` must preserve the generated `certs-manifest.json` field layout, because `write_dev_init_bundle(...)` writes that manifest to disk and tests assert the current output shape.

**Planning-Time Verification Summary:**
- `#28`: still live. [`crates/remote-exec-broker/src/port_forward/supervisor.rs`](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-broker/src/port_forward/supervisor.rs:28) still defines `ForwardRuntime` as a 10-field mixed identity/context bag, and the same type is constructed or consumed across `supervisor/open.rs`, `supervisor/reconnect.rs`, `tcp_bridge.rs`, `udp_bridge.rs`, and broker test helpers.
- `#29`: still live. [`crates/remote-exec-proto/src/sandbox.rs`](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-proto/src/sandbox.rs:1) remains a 401-line module that combines configuration types, authorization logic, canonicalization/path helpers, and inline tests; broker, daemon, and host crates all import the current root module surface.
- `#30`: still live, with an added compatibility constraint. [`crates/remote-exec-pki/src/manifest.rs`](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-pki/src/manifest.rs:19) still duplicates `cert_pem` and `key_pem` inside `DaemonManifestEntry` instead of composing `KeyPairPaths`, and [`crates/remote-exec-pki/src/write/bundle.rs`](/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-pki/src/write/bundle.rs:58) still writes the serialized manifest to `certs-manifest.json`, so the cleanup cannot silently change that on-disk shape.

---

### Task 1: Broker Port-Forward Runtime Identity Split

**Intent:** Separate forward identity data from operational handles inside the broker port-forward supervisor without changing broker behavior, reconnect flow, or test expectations.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/reconnect.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/udp_bridge.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/test_support.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/store.rs`

**Notes / constraints:**
- Preserve all current `ForwardPortEntry` bookkeeping, reconnect state transitions, and warning/logging content unless execution proves a tiny wording change is unavoidable.
- Do not reopen the earlier Phase 3 ownership changes around listen-session control, reconnect deadlines, or tunnel-close semantics.
- Keep the split structural: the point is to clarify which data is stable identity versus which handles are mutable runtime context.

**Verification:**
- Run: `cargo test -p remote-exec-broker port_forward:: -- --nocapture`
- Expect: broker unit tests covering supervisor, TCP, and UDP forwarding still pass after the internal type split.
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: public broker forwarding behavior stays unchanged end-to-end on the Rust path.

- [ ] Inspect every current `ForwardRuntime` constructor and consumer to confirm which fields are pure identity versus runtime handles
- [ ] Introduce an internal identity type and refactor the runtime/state holders around it without changing broker behavior
- [ ] Update test helpers and any logging/store call sites that currently depend on the mixed bag shape
- [ ] Run focused broker verification for unit and integration forwarding paths
- [ ] Commit with real changes only

### Task 2: Proto Sandbox Module Boundary Split

**Intent:** Split the proto sandbox module into smaller files for types, authorization, and reusable path helpers while preserving the current public module API and behavior.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/lib.rs`
- Likely replace: `crates/remote-exec-proto/src/sandbox.rs`
- Likely create: `crates/remote-exec-proto/src/sandbox/mod.rs`
- Likely create: `crates/remote-exec-proto/src/sandbox/types.rs`
- Likely create: `crates/remote-exec-proto/src/sandbox/authorize.rs`
- Likely create: `crates/remote-exec-proto/src/sandbox/path_utils.rs`
- Existing references: `crates/remote-exec-broker/src/startup.rs`
- Existing references: `crates/remote-exec-broker/src/local_transfer.rs`
- Existing references: `crates/remote-exec-daemon/src/config/mod.rs`
- Existing references: `crates/remote-exec-host/src/exec/support.rs`
- Existing references: `crates/remote-exec-host/src/transfer/`

**Notes / constraints:**
- Keep `remote_exec_proto::sandbox::FilesystemSandbox`, `CompiledFilesystemSandbox`, `SandboxAccess`, `SandboxError`, `compile_filesystem_sandbox(...)`, and `authorize_path(...)` available at the current root import path.
- Preserve canonicalization, deny/allow ordering, case-sensitivity behavior, and all current error strings unless execution proves a tiny text change is unavoidable and tests are updated deliberately.
- Keep the split local to `remote-exec-proto`; do not widen into a cross-crate path utility reshuffle unless the current file move requires a tiny shared re-export.

**Verification:**
- Run: `cargo test -p remote-exec-proto sandbox::tests -- --nocapture`
- Expect: direct sandbox behavior stays unchanged after the split.
- Run: `cargo test -p remote-exec-daemon --test patch_rpc`
- Expect: daemon patch sandbox enforcement still passes.
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: daemon transfer sandbox behavior still passes.
- Run: `cargo test -p remote-exec-broker --test mcp_assets`
- Expect: broker local patch/image behavior still passes with the stable sandbox exports.
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: broker local transfer sandbox behavior still passes end-to-end.

- [ ] Confirm the exact split between configuration types, authorization flow, reusable path helpers, and inline tests
- [ ] Move the sandbox implementation into a module tree with stable root-level re-exports
- [ ] Update downstream imports only where necessary to accommodate the internal module move
- [ ] Run focused proto, daemon, and broker verification for sandboxed paths
- [ ] Commit with real changes only

### Task 3: PKI Manifest Path Composition Cleanup

**Intent:** Remove the duplicated daemon cert/key path fields from `DaemonManifestEntry` by composing `KeyPairPaths`, while preserving the serialized manifest and snippet output seen by users and tests.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-pki/src/manifest.rs`
- Likely modify: `crates/remote-exec-pki/src/lib.rs`
- Likely modify: `crates/remote-exec-pki/src/write/bundle.rs`
- Likely modify: `crates/remote-exec-pki/tests/dev_init_bundle.rs`
- Likely modify: `crates/remote-exec-admin/tests/dev_init.rs`
- Existing references: `crates/remote-exec-admin/src/certs.rs`
- Existing references: `crates/remote-exec-pki/src/write.rs`

**Notes / constraints:**
- Preserve the on-disk `certs-manifest.json` field names for daemon entries unless the user explicitly asks for a manifest format migration.
- Preserve `render_config_snippets(...)` output, including the current cert/key path lines and TOML snippet shape.
- If `DaemonManifestEntry` changes internal field layout, use `serde(flatten)` or an equivalent compatibility-safe approach so JSON consumers and tests remain stable.

**Verification:**
- Run: `cargo test -p remote-exec-pki manifest::tests -- --nocapture`
- Expect: snippet rendering still matches the current TOML and path expectations.
- Run: `cargo test -p remote-exec-pki --test dev_init_bundle -- --nocapture`
- Expect: bundle generation and manifest persistence still pass with the composed shape.
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Expect: admin CLI dev-init flow still writes the expected bundle and manifest files.

- [ ] Confirm every current `DaemonManifestEntry` constructor, serializer, and test assertion that depends on the duplicated fields
- [ ] Refactor the manifest type to compose `KeyPairPaths` while preserving serialized compatibility and snippet output
- [ ] Update tests and any convenience access patterns to the new internal structure
- [ ] Run focused PKI and admin verification for manifest and bundle generation
- [ ] Commit with real changes only

### Task 4: Final Phase 5 Confirmatory Sweep

**Intent:** Reconfirm which structural Phase 5 seams were still live, verify the compatibility-scoped implementations, and finish with the Rust workspace quality gate.

**Relevant files/components:**
- Likely inspect: `docs/CODE_AUDIT_ROUND5.md`
- Likely inspect: the broker, proto, and PKI files touched by Tasks 1 through 3
- Likely inspect: any updated tests under `crates/remote-exec-broker`, `crates/remote-exec-proto`, `crates/remote-exec-pki`, and `crates/remote-exec-admin`

**Notes / constraints:**
- Keep the completion notes explicit where the implementation is intentionally compatibility-scoped, especially for `#30` and the serialized manifest shape.
- Re-run targeted searches so the closeout distinguishes “implemented”, “narrowed by compatibility constraints”, and “kept stable by root-module re-exports”.
- No C++ verification is required unless execution unexpectedly touches a shared C++-facing contract or documentation surface.

**Verification:**
- Run: `cargo test --workspace`
- Expect: the Rust workspace passes end-to-end after the full Phase 5 bundle.
- Run: `cargo fmt --all --check`
- Expect: formatting stays clean.
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: no lint regressions after the structural refactors.

- [ ] Re-run targeted searches for items `#28` through `#30` and confirm which fixes were exact versus compatibility-scoped
- [ ] Run the full Rust quality gate for the combined structural refactor
- [ ] Summarize the compatibility-scoped result for `#30` if the serialized manifest shape stays intentionally stable
- [ ] Commit any sweep-only adjustments if needed; otherwise do not create an empty commit
