# Apply Patch Direct Parity Design

Status: approved design captured in writing

Date: 2026-04-01

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/src/tools/patch.rs`
- `crates/remote-exec-broker/tests/mcp_assets.rs`
- `crates/remote-exec-daemon/src/patch/mod.rs`
- `crates/remote-exec-daemon/src/patch/parser.rs`
- `crates/remote-exec-daemon/src/patch/engine.rs`
- `crates/remote-exec-daemon/tests/patch_rpc.rs`
- `crates/remote-exec-proto/src/public.rs`

## Goal

Bring the direct `apply_patch` tool behavior closer to the updated Codex compatibility notes without changing the repo's runtime architecture.

This batch focuses only on externally observable direct-tool behavior:

- pre-verification before mutation
- direct-tool success shape of summary text plus empty structured content
- direct-tool failure semantics that surface earlier and more consistently

## Scope

Included:

- keep `apply_patch` as a standard MCP function tool with JSON input `{ "input": "<patch>" }`
- add a full daemon-side verification phase before the first filesystem mutation
- make successful broker `apply_patch` calls return summary text plus `{}` structured content
- preserve current add, update, delete, move, newline, and summary behaviors where they already match the compatibility notes
- extend tests to prove earlier semantic failures happen before mutation begins

Excluded:

- freeform/custom-tool transport support
- `exec_command` interception of disguised `apply_patch`
- implicit-invocation rejection for shell/argv forms
- path restriction hardening
- patch begin/end events
- runtime self-invocation or external patch subprocess execution
- approval or sandbox behavior

## Compatibility Interpretation

This repository currently exposes tools through RMCP function-tool plumbing. For this batch, "direct `apply_patch` parity" means matching Codex's externally observable direct-tool behavior on that existing MCP surface, rather than reproducing Codex's internal freeform/custom-tool transport literally.

That means:

- the canonical public entry point stays `apply_patch({ target, input, workdir? })`
- model-facing success remains plain text summary
- structured success content becomes an empty object
- semantic failures move into a dedicated pre-verification phase before mutation

## Current Behavior Summary

Today the daemon parses the patch and then applies each action immediately in sequence.

That leaves two important gaps:

1. semantic failures are discovered lazily during mutation
2. successful direct `apply_patch` calls return a non-empty structured object from the broker

The lazy mutation path means a later failure can happen only after earlier actions have already changed the filesystem, even when that failure was knowable from validation alone.

Examples of currently lazy failures:

- delete target is a directory
- update source file is missing
- update/delete source file is unreadable as UTF-8
- update hunk context cannot be found in a later file

The broker-side output also diverges from the updated compatibility notes by returning `{ target, output }` structured content instead of an empty object for successful direct `apply_patch`.

## Decision Summary

### 1. Keep the MCP function-tool entry point

For this batch, the broker continues to expose only the current JSON function-tool form:

- tool name: `apply_patch`
- input: `{ "target": "...", "input": "<patch>", "workdir"?: "..." }`

This is a deliberate repo-local compatibility interpretation. It avoids transport churn while we fix the more important direct observable behavior.

### 2. Add daemon-side pre-verification before mutation

The daemon should split patch handling into two phases:

1. verification
2. execution

Verification responsibilities:

- parse the patch text into actions
- resolve the effective cwd from `workdir`
- compute a verified execution plan for all actions before writing any file
- force update and delete actions to read required source files during verification
- force update hunks to match and compute final rewritten text during verification
- reject semantic failures before the first filesystem mutation

Execution responsibilities:

- execute the already-verified plan sequentially
- keep the existing non-atomic behavior once execution has started
- emit the same git-style success summary text on success

This preserves existing semantics that already match the notes while moving predictable failures earlier.

### 3. Return empty structured success content for direct `apply_patch`

Successful broker `apply_patch` calls should return:

- model-facing summary text
- empty structured content `{}` rather than `{ target, output }`

Failures continue to surface as tool errors.

This aligns the broker's observable direct-tool behavior with the updated notes without changing the daemon response body shape.

## Rejected Alternatives

### Full freeform/custom-tool support in this batch

This would try to reproduce Codex's freeform `apply_patch` transport literally.

It was rejected because the repo's current RMCP plumbing is function-tool oriented, and transport rework would distract from the more valuable observable behavior fixes in this batch.

### Runtime self-invocation of an apply-patch subprocess

This would move the daemon away from its current in-process patch execution and toward a more Codex-like runtime wrapper.

It was rejected because the user explicitly scoped this batch to externally observable behavior only.

### Path restriction hardening now

This would reject absolute paths and `..` traversal during patch verification.

It was rejected because the user explicitly asked to skip path restriction in this batch.

## Code Boundaries

### `crates/remote-exec-daemon/src/patch/parser.rs`

- keep this as the syntax parser for patch text
- do not expand it into a semantic verifier

### `crates/remote-exec-daemon/src/patch/engine.rs`

- keep this as the update-hunk application engine
- reuse it during verification to compute final updated file text

### `crates/remote-exec-daemon/src/patch/mod.rs`

- stop applying parsed actions directly after parse
- orchestrate:
  - cwd resolution
  - parse
  - verification
  - execution of verified actions
  - success summary generation

### `crates/remote-exec-daemon/src/patch/verify.rs`

- new focused verifier module
- build a concrete verified execution plan before mutation
- front-load text reads, hunk matching, and delete-target checks

### `crates/remote-exec-broker/src/tools/patch.rs`

- keep forwarding requests to the daemon
- change successful structured output to `{}` while preserving summary text content

### `crates/remote-exec-proto/src/public.rs`

- remove or stop using `ApplyPatchResult`
- keep request input shape unchanged

### `crates/remote-exec-broker/tests/mcp_assets.rs`

- update the direct `apply_patch` success expectation to require empty structured content

### `crates/remote-exec-daemon/tests/patch_rpc.rs`

- add daemon tests that prove semantic verification failures happen before any mutation begins

## Verified Plan Shape

The verifier should produce concrete execution records instead of re-reading source files during mutation.

Proposed shape:

- `VerifiedAction::Add`
  - absolute target path
  - final file content to write
  - summary entry path
- `VerifiedAction::Delete`
  - absolute target path
  - summary entry path
- `VerifiedAction::Update`
  - absolute source path
  - optional absolute move destination
  - final rewritten text
  - summary entry path

This keeps execution simple and makes it impossible for expected semantic failures to appear only after writes have started.

## Data Flow

### Direct `apply_patch` success path

1. broker receives `apply_patch({ target, input, workdir? })`
2. broker forwards patch text and workdir to the daemon
3. daemon resolves effective cwd
4. daemon parses the patch
5. daemon verifies every action and builds a verified execution plan
6. daemon executes the verified plan sequentially
7. daemon returns the summary text
8. broker returns summary text to the model plus `{}` structured content

### Direct `apply_patch` failure path

1. broker forwards the patch request to the daemon
2. daemon fails during parse or verification before mutation starts
3. daemon returns `patch_failed`
4. broker surfaces the error as a tool failure

## Error Handling

- Keep `patch_failed` as the daemon-visible error code for parse and semantic verification failures in this batch.
- Parse and verification failures should occur before the first filesystem mutation.
- Execution-time filesystem failures may still leave partial effects once execution has begun.
- Delete-target verification must reject directories before execution.
- Update and delete verification must read source files as UTF-8 text up front and fail early if they cannot be read that way.
- Add-file behavior remains overwrite-on-write and is still classified as `A`.

## Testing Plan

### Broker tests

Update direct-tool output coverage to prove:

- summary text still contains `Success. Updated the following files:`
- successful structured content is exactly `{}`

### Daemon tests

Add focused regression coverage for:

- a later verification failure preventing an earlier update from mutating disk
- delete-of-directory rejection before any earlier mutation
- non-UTF-8 update/delete source rejection before any earlier mutation

Preserve existing coverage for:

- overwrite-on-add behavior
- EOF marker behavior
- repeated-context additions
- non-atomic behavior once execution has genuinely started

## Success Criteria

This batch is complete when:

- successful direct `apply_patch` calls return summary text plus empty structured content
- parse and semantic verification failures occur before the first filesystem mutation
- directory-delete and non-UTF-8 update/delete cases fail during verification rather than mid-mutation
- current matching and summary behaviors that already align with the notes stay intact
- no path restriction hardening, interception, or event work is introduced in this batch
