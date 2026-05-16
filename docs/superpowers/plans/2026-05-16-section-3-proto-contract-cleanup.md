# Section 3 Proto Contract Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Clean up the verified section 3 proto/schema smells that improve boundary clarity and internal API hygiene without causing avoidable public contract churn.

**Requirements:**
- Fix the verified and worthwhile section 3 items from `docs/code-quality-audit-2026-05-16.md`: a narrowed `3.3`, `3.4`, `3.5`, and `3.7`.
- Treat `3.2` as intentionally out of scope for this pass: removing `ForwardPortsResult.action` would churn the public `forward_ports` structured result with very little value.
- Treat `3.6` as intentionally out of scope for this pass: `wire.rs` is tiny, but it is also a shared helper used by multiple wire-mapped enums, so deleting it would mostly trade one small module for local duplication.
- Preserve the public MCP tool surface, broker-daemon wire format, and current `forward_ports` result shape.
- Keep `ForwardPortProtocol` and `TunnelForwardProtocol` as distinct public-vs-tunnel boundary types; do not collapse them into one shared enum type across public and private layers.
- Keep this pass Rust-focused. Do not redesign the C++ `path_policy` API in the same change.

**Architecture:** The right cleanup here is to make the proto crate more authoritative at its own boundaries without flattening intentionally separate layers. For forward-port protocol types, keep the public and tunnel enums distinct but centralize the conversion/parity seam inside `remote-exec-proto` so broker code stops open-coding the mapping. For paths, move the Rust API toward type-owned methods on `PathPolicy` and migrate internal Rust callers to the method form, while deciding during implementation whether free-function wrappers can be removed cleanly or should remain as thin compatibility shims. The remaining low-risk cleanup is to delete dead proto exports and flatten tiny re-export-only modules where that improves locality without changing behavior.

**Verification Strategy:** Run `remote-exec-proto` tests for the direct API changes, then focused broker and host tests that exercise the affected protocol and path-policy call sites. Finish with `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

**Assumptions / Open Questions:**
- `ForwardPortsAction` is still a real redundancy, but the result field is part of the public broker structured-content shape and is used directly by broker formatting today. Unless the user explicitly wants that contract churn, this pass should leave it intact.
- `EmptyResponse` appears unused across the current workspace. Implementation should confirm that remains true before deleting the type and re-export.
- For `PathPolicy`, the preferred end state is method-based internal Rust call sites. Implementation should decide whether to retain the existing free functions as wrappers based on the actual amount of churn and whether any remaining internal call sites still need them.

---

### Task 1: Centralize forward-port protocol conversions without collapsing the boundary

**Intent:** Keep the public and tunnel protocol enums separate, but make their relationship authoritative inside `remote-exec-proto` instead of leaving broker code to hand-write conversions.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/public.rs`
- Likely modify: `crates/remote-exec-proto/src/port_tunnel/meta.rs`
- Likely modify: `crates/remote-exec-proto/src/port_tunnel/mod.rs`
- Likely modify: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs`
- Existing references: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Existing references: `crates/remote-exec-host/src/port_forward/*`

**Notes / constraints:**
- Do not merge `ForwardPortProtocol` and `TunnelForwardProtocol` into a single type. The public broker tool surface and the daemon-private tunnel layer are intentionally different boundaries.
- Prefer `From`/`Into` or similarly explicit conversion helpers defined in proto over broker-local mapping functions such as `tunnel_protocol(...)`.
- Add parity tests that make it obvious if a future protocol variant is added to one enum but not the other.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Expect: public and tunnel protocol mapping becomes centralized, and forward-port behavior remains unchanged.

- [ ] Confirm the current open-coded public-to-tunnel protocol mapping seams
- [ ] Add authoritative conversion/parity helpers in `remote-exec-proto` without collapsing the two boundary types
- [ ] Replace broker-local conversion helpers with the shared proto seam
- [ ] Add or update tests that fail loudly if the protocol enums drift apart
- [ ] Run focused verification
- [ ] Commit

### Task 2: Add `PathPolicy` methods and migrate internal Rust callers

**Intent:** Move the Rust path-policy API toward type-owned methods so internal call sites stop passing `PathPolicy` around as a C-style tag parameter.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/path.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer/endpoints.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- Likely modify: `crates/remote-exec-broker/src/local_transfer.rs`
- Likely modify: `crates/remote-exec-host/src/host_path.rs`
- Likely modify: `crates/remote-exec-host/src/sandbox.rs`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/mod.rs`

**Notes / constraints:**
- Prefer inherent methods on `PathPolicy`; do not introduce a separate `PathOps` trait in this pass.
- Keep the semantics of Windows-drive translation, path joining, basename extraction, and syntax-only equality unchanged.
- This is a Rust-internal ergonomics cleanup. Do not mirror the API change into the C++ daemon in the same pass.
- If removing the old free functions causes unnecessary churn or awkward migration sequencing, keep them temporarily as thin wrappers and migrate internal Rust code first.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Expect: path normalization and transfer path behavior remain unchanged while internal Rust call sites become method-based.

- [ ] Add the `PathPolicy` method surface and confirm the exact wrapper-retention strategy
- [ ] Migrate internal Rust call sites from the free-function form to the method form
- [ ] Keep or remove the old free functions based on the verified migration shape, without changing semantics
- [ ] Run focused verification
- [ ] Commit

### Task 3: Remove dead proto exports and flatten tiny re-export-only modules

**Intent:** Clean up the low-risk proto holdouts that are real but do not require public wire-shape churn.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/rpc/image.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc.rs`
- Likely modify: `crates/remote-exec-proto/src/sandbox.rs`
- Likely delete: `crates/remote-exec-proto/src/sandbox/types.rs`
- Existing references: workspace-wide imports of `remote_exec_proto::rpc::*`
- Existing references: workspace-wide imports of `remote_exec_proto::sandbox::*`

**Notes / constraints:**
- Remove `EmptyResponse` only after confirming it has no meaningful workspace use.
- Flatten `sandbox.rs` and `sandbox/types.rs` into one file if that remains a pure locality cleanup with no import or serialization behavior change.
- Leave `wire.rs` alone in this task even though it is small; the point is to remove dead or purely indirection-only proto pieces, not to chase file-count minimalism blindly.

**Verification:**
- Run: `cargo test -p remote-exec-proto`
- Run: `cargo test -p remote-exec-daemon --test image_rpc`
- Run: `cargo fmt --all --check`
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Expect: dead exports are gone, sandbox types still serialize and deserialize identically, and the workspace passes format and lint gates.

- [ ] Confirm `EmptyResponse` has no real workspace usage before deleting it
- [ ] Remove the dead image RPC export if it remains unused
- [ ] Flatten the tiny sandbox re-export module into one source file
- [ ] Keep `wire.rs` explicitly unchanged in this pass
- [ ] Run focused verification and final quality gates
- [ ] Commit
