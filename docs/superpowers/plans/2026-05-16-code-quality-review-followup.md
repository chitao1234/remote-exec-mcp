# Code Quality Review Follow-Up Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-valid issues from `docs/code-quality-review.md` by tightening typed Rust boundaries, removing avoidable public/config duplication, and cleaning up the remaining C++ internal contract and supervision seams.

**Requirements:**
- Preserve the public wire format unless a task explicitly calls out a compatibility-preserving alias or deprecation path.
- Keep broker-owned public IDs, target isolation, and port-forward v4 semantics unchanged.
- Do not broaden the scope into unrelated feature work or generalized framework building.
- Keep Rust daemon and C++ daemon behavior aligned where they share the same public or broker-daemon contract.
- Prefer single-source internal inventories and typed adapters over repeated string literals and piecemeal normalization logic.

**Architecture:** The work should converge on three clearer boundaries. First, Rust host and daemon code should keep typed errors and shared capability/config models until the transport edge instead of reconstructing meaning from strings. Second, public transfer/config surfaces should normalize around one canonical internal model with thin compatibility adapters at the broker edge. Third, the C++ daemon should use one internal contract inventory for routes/constants and one explicit supervision owner for port-forward workers, rather than scattered literals and detached execution seams.

**Verification Strategy:** Use focused broker/daemon/C++ tests for each task, plus Windows cross-target compile gates when touching shared host code or C++ parity-sensitive areas. End with example-config smoke coverage and a broader workspace verification pass proportional to the files touched.

**Assumptions / Open Questions:**
- Keep `transfer_files.source` as a broker-side compatibility alias until the user explicitly approves removing it from the public input schema.
- Confirm during implementation whether `view_image.detail` cleanup belongs in this plan or should stay out of scope; the review follow-up below does not require it.
- Confirm whether the shared test-support extraction should live under `tests/support/` as a dev-only crate or under `crates/` with `publish = false`.

---

### Task 1: Finish typed Rust error boundaries

**Intent:** Remove the remaining stringly and `anyhow`-heavy error seams that still sit between the host runtime and daemon transport boundary.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-host/src/error.rs`
- Likely modify: `crates/remote-exec-host/src/patch/`
- Likely modify: `crates/remote-exec-host/src/transfer/archive/`
- Likely modify: `crates/remote-exec-daemon/src/rpc_error.rs`
- Existing references: `crates/remote-exec-proto/src/rpc/error.rs`

**Notes / constraints:**
- Preserve the existing external RPC error codes and status mappings.
- Reuse the already-typed transfer/image error style instead of inventing a parallel model.
- Avoid a full error-ecosystem rewrite; the goal is to finish the remaining holdouts.

**Verification:**
- Run: `cargo test -p remote-exec-host`
- Run: `cargo test -p remote-exec-daemon --test patch_rpc`
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Expect: existing RPC error codes remain stable while internal conversions become simpler and more typed.

- [x] Inspect the remaining `HostRpcError`, patch, and archive error seams and confirm the minimal typed model to use
- [x] Introduce typed patch/archive error paths and keep `RpcErrorCode` typed internally
- [x] Remove avoidable `String`/`anyhow` reconstruction from the daemon-facing host boundary
- [x] Run focused Rust verification
- [x] Commit

### Task 2: Consolidate shared capability and transfer modeling

**Intent:** Reduce manual duplication in daemon identity/capability data and normalize transfer metadata around one canonical internal model.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/public.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/target.rs`
- Likely modify: `crates/remote-exec-proto/src/rpc/transfer.rs`
- Likely modify: `crates/remote-exec-proto/src/transfer.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer.rs`
- Likely modify: `crates/remote-exec-daemon/src/transfer/codec.rs`
- Existing references: `crates/remote-exec-host/src/state.rs`

**Notes / constraints:**
- Shared capability structs should not accidentally expose new public broker fields unless that is intentional.
- Normalize internal models first; keep compatibility shims at the broker edge where needed.
- Avoid changing transport semantics for transfer compression, symlink mode, or overwrite behavior.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
- Run: `cargo test -p remote-exec-broker --test multi_target -- --nocapture`
- Expect: no behavioral drift in transfer behavior, and capability data should come from shared typed structs instead of duplicated parallel definitions.

- [x] Extract shared daemon identity/capability structs in `remote-exec-proto`
- [x] Introduce one canonical transfer metadata/envelope model and keep HTTP header encoding as an adapter layer
- [x] Update broker and daemon code to consume the shared types instead of parallel local shapes
- [x] Run focused transfer and capability verification
- [x] Commit

### Task 3: Remove legacy public-shape and config duplication

**Intent:** Simplify the public transfer input shape and reduce daemon/broker-host config mirroring so host runtime settings are composed rather than re-declared.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-proto/src/public.rs`
- Likely modify: `crates/remote-exec-broker/src/bin/remote_exec.rs`
- Likely modify: `crates/remote-exec-broker/src/tools/transfer.rs`
- Likely modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Likely modify: `crates/remote-exec-host/src/config/mod.rs`
- Likely modify: `configs/broker.example.toml`
- Existing references: `crates/remote-exec-broker/src/config.rs`

**Notes / constraints:**
- Keep `source` as a compatibility alias unless the user explicitly approves removing it.
- Preserve config-file compatibility where practical; prefer composition and validation cleanup over churn in end-user syntax.
- The example-config/default cleanup should be verified by tests rather than only docs edits.

**Verification:**
- Run: `cargo test -p remote-exec-broker --test mcp_transfer`
- Run: `cargo test -p remote-exec-broker --test mcp_cli`
- Run: `cargo test -p remote-exec-daemon --test health`
- Expect: one canonical broker-side transfer input path, less config field copying, and example/default behavior covered by tests.

- [x] Normalize broker and CLI handling around `sources`, keeping `source` as a compatibility adapter if still required
- [x] Refactor daemon and embedded-host config composition to reduce field-by-field mirroring
- [x] Add example-config/default-behavior smoke coverage and clarify intentional non-default examples
- [x] Run focused broker and daemon verification
- [x] Commit

### Task 4: Move repository-specific bootstrap rendering out of `remote-exec-pki`

**Intent:** Restore `remote-exec-pki` to a reusable certificate/manifest role and move repository-specific operator UX into the admin CLI layer.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-pki/src/lib.rs`
- Likely modify: `crates/remote-exec-pki/src/manifest.rs`
- Likely modify: `crates/remote-exec-admin/src/certs.rs`
- Likely create: `crates/remote-exec-admin/src/bootstrap/` or equivalent helper module
- Existing references: `README.md`, `configs/*.example.toml`

**Notes / constraints:**
- Keep machine-readable manifest output in `remote-exec-pki`.
- Move only repo-specific snippet rendering and bootstrap prose, not certificate generation or secure write logic.
- Preserve current CLI operator outcomes unless the output wording must change to reflect the new ownership boundary.

**Verification:**
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Run: `cargo test -p remote-exec-admin --test certs_issue`
- Run: `cargo test -p remote-exec-pki --test dev_init_bundle`
- Expect: the admin CLI still emits the expected bootstrap guidance, while the PKI crate stops owning repo-specific snippet rendering.

- [x] Split manifest/data concerns from bootstrap rendering concerns
- [x] Move snippet rendering and operator-facing text into `remote-exec-admin`
- [x] Keep `remote-exec-pki` limited to reusable certificate and manifest responsibilities
- [x] Run focused admin and PKI verification
- [x] Commit

### Task 5: Introduce a C++ daemon route registry and shared contract constants

**Intent:** Replace the current C++ daemon route/constant scattering with one internal contract inventory used by routing, transfer, and upgrade handling.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/server_route_common.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/http_connection.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/transfer_http_codec.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Likely create: `crates/remote-exec-daemon-cpp/src/http_route_registry.*` or `server_contract.*`

**Notes / constraints:**
- This is an internal C++ cleanup; do not change public paths, methods, or protocol versions.
- Keep the abstraction narrow and data-driven. One contract inventory is enough; do not build a generic framework.
- Reuse the repo’s existing route handlers rather than rewriting their business logic.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-runtime`
- Run: `make -C crates/remote-exec-daemon-cpp test-host-server-streaming`
- Expect: route dispatch and shared constants come from one internal source without changing observable behavior.

- [x] Define one internal route/contract inventory for the C++ daemon
- [x] Move shared path/header/version literals to that inventory and wire existing handlers through it
- [x] Update route/transport/codec code to consume the shared contract layer
- [x] Run focused C++ verification
- [x] Commit

### Task 6: Replace remaining detached worker seams and clean up test support

**Intent:** Finish the C++ port-forward supervision model and lower maintenance cost by promoting shared test helpers into a clearer reusable seam.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_spawn.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_service.*`
- Likely modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_session.cpp`
- Likely modify: `crates/remote-exec-broker/tests/support/mod.rs`
- Likely modify: `crates/remote-exec-daemon/tests/support/mod.rs`
- Likely modify: `tests/support/`
- Likely create: a small dev-only shared test-support crate

**Notes / constraints:**
- Keep the existing `PortTunnelService` ownership model and complete it; do not start another full port-forward redesign.
- Preserve current reconnect/shutdown semantics while replacing detached execution with tracked supervision.
- Shared test-support extraction should reduce `#[path = ...]` includes and large scenario-file coupling without forcing a test rewrite all at once.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
- Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
- Run: `cargo test -p remote-exec-daemon --test port_forward_rpc`
- Expect: C++ port-forward workers have explicit supervision, and test-support reuse becomes more structural and less file-include driven.

- [x] Replace detached or untracked C++ port-forward worker startup with tracked supervision under `PortTunnelService`
- [x] Preserve teardown, budget, and reconnect semantics while tightening failure accounting
- [x] Extract shared Rust test helpers into a clearer dev-only support seam and reduce direct `#[path = ...]` inclusion
- [x] Run focused C++ and broker/daemon port-forward verification
- [x] Commit

### Task 7: Final drift sweep

**Intent:** Verify that the cleanup work removed the intended duplication without creating new behavior drift across docs, examples, public schemas, and tests.

**Relevant files/components:**
- Likely modify: `README.md`
- Likely modify: `configs/*.example.toml`
- Likely modify: `skills/using-remote-exec-mcp/SKILL.md`
- Existing references: public schemas under `crates/remote-exec-proto/src/`

**Notes / constraints:**
- Only update user-facing docs that are directly affected by the implemented refactors.
- Keep this as a sweep and verification task, not a new source of unrelated polish.

**Verification:**
- Run: `cargo test --workspace`
- Run: `cargo fmt --all --check`
- Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- Run: relevant C++ checks for touched C++ code
- Expect: docs/examples/tool help align with the cleaned-up implementation and the workspace passes its normal quality gates.

- [ ] Sweep the affected docs/examples/skill text for newly stale wording
- [ ] Run the full relevant quality gates for the touched Rust and C++ surfaces
- [ ] Fix any remaining drift or verification failures discovered by the sweep
- [ ] Run final verification again
- [ ] Commit
