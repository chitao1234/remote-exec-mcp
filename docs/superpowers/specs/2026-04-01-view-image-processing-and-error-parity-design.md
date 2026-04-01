# View Image Processing And Error Parity Design

Status: approved design captured in writing

Date: 2026-04-01

References:

- `docs/local-system-tools.md`
- `docs/specs/2026-03-31-remote-exec-mcp-design.md`
- `crates/remote-exec-daemon/src/image.rs`
- `crates/remote-exec-daemon/tests/image_rpc.rs`
- `crates/remote-exec-broker/src/tools/image.rs`
- `crates/remote-exec-broker/tests/mcp_assets.rs`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`

## Goal

Bring the standalone `view_image` behavior closer to the updated Codex compatibility notes for:

- image processing and encoding behavior
- visible error strings and failure shape

This batch intentionally stops short of full `view_image` parity. It does not change the repo-local decision to always expose the tool and it does not attempt to add `detail` to the model-facing `input_image` content item.

## Scope

Included:

- default resize-to-fit behavior that matches the documented Codex rules more closely
- passthrough of original bytes for PNG, JPEG, and WebP when default mode does not require resizing
- resize-time re-encoding behavior aligned with the documented format rules
- non-passthrough GIF handling
- Codex-like error wording for invalid detail, missing file, directory path, and non-image or processing failures
- broker and daemon tests proving those externally visible behaviors

Excluded:

- model-capability gating for `view_image`
- conditional schema exposure of `detail`
- adding `detail` to the model-facing `input_image` content item
- event parity
- image cache parity
- transport or schema redesign for `view_image`

## Compatibility Interpretation

There are two relevant compatibility sources here:

1. the updated Codex runtime notes in `docs/local-system-tools.md`
2. the repo-local v1 design in `docs/specs/2026-03-31-remote-exec-mcp-design.md`

This batch follows both by:

- aligning processing, encoding, and failure behavior with the Codex notes
- preserving the repo-local decision that `view_image` remains always exposed and still returns broker-specific structured content with `target`

That means the parity target for this batch is:

- default and `"original"` processing behavior
- visible data URL encoding behavior
- visible error messages and failure shape

It is not:

- full tool-registration parity with Codex
- full content-item parity for optional `detail`

## Current Behavior Summary

Today the broker and daemon already match some of the intended behavior:

- `view_image` returns one `input_image` content item plus structured JSON
- `detail` accepts omitted, `null`, or `"original"`
- default mode uses resize-to-fit bounds of `2048x768`
- `"original"` preserves original bytes for the currently supported formats
- invalid `detail` is rejected

The main remaining mismatches are:

1. default-mode processing always re-encodes to PNG, even when the source image already fits within bounds and the source format is PNG, JPEG, or WebP
2. default-mode processing does not preserve original bytes for already-small PNG, JPEG, or WebP images
3. GIF handling is currently just passthrough in `"original"` mode and not deliberately distinguished in default mode
4. visible error strings do not yet match the path-context wording documented in the Codex notes
5. broker-side invalid-detail rejection currently uses a shorter message than the documented daemon-level wording

## Decision Summary

### 1. Keep the current broker/daemon split

The daemon remains the place where image reading, decoding, sizing, and encoding happen.

The broker remains responsible for:

- target validation
- daemon routing
- wrapping the daemon response into MCP image content plus structured content

This keeps the batch focused on observable behavior, not architecture churn.

### 2. Match Codex-style default processing rules

The daemon should distinguish between:

- default resized behavior
- explicit `"original"` behavior

For default behavior:

- if the image already fits within `2048x768`, do not resize it
- if the original format is PNG, JPEG, or WebP and no resize is needed, return the original file bytes directly
- if resize is required, resize to fit using `FilterType::Triangle`
- when re-encoding resized output, prefer the format family documented in the notes rather than always forcing PNG

For `"original"` behavior:

- preserve original resolution
- return original bytes for supported passthrough formats

### 3. Treat GIF as processable input, not a passthrough contract

The compatibility notes say GIF is recognized as an input format but is not preserved byte-for-byte.

For this batch:

- GIF remains an accepted input format
- default-mode behavior should decode and process it rather than treat it like passthrough PNG/JPEG/WebP
- `"original"` does not need to preserve GIF bytes byte-for-byte as part of the external parity target

The key point is to avoid implicitly treating GIF as a guaranteed passthrough format.

### 4. Align error wording at the visible tool surface

The user-visible `view_image` error surface should move closer to the Codex notes.

Target wording categories:

- invalid detail:
  - `view_image.detail only supports `original`; omit `detail` for default resized behavior, got `<value>``
- missing file:
  - `unable to locate image at `<abs-path>`: ...`
- directory path:
  - `image path `<abs-path>` is not a file`
- non-image or processing failure:
  - `unable to process image at `<abs-path>`: ...`

This requires two specific changes:

- daemon-side errors should wrap filesystem and image-processing failures with path-context wording
- broker-side invalid-detail rejection should be removed or aligned so the final visible message is the full Codex-style wording

### 5. Keep failure shape unchanged

On failure:

- the tool call should still fail normally
- the broker should still return ordinary text error content
- no image content item should be emitted

That already matches the documented failure shape, so the batch only needs to preserve it while changing wording.

## Rejected Alternatives

### Full `view_image` parity in one batch

This would include:

- model-capability gating
- conditional schema exposure of `detail`
- model-facing `input_image.detail`
- processing and error parity

It was rejected because the user explicitly asked to leave the content-item `detail` and gating mismatches out of scope, and the repo already has a deliberate v1 policy for always exposing `view_image`.

### Broker-only fixes

This would try to patch visible behavior by changing only broker wrapping logic.

It was rejected because the main mismatches live in daemon-side decode, resize, passthrough, and error construction. The broker alone cannot fix the underlying processing rules.

### Preserve current always-PNG default behavior

This would keep default-mode output simple by always re-encoding to PNG.

It was rejected because it directly contradicts the documented Codex behavior for already-small PNG/JPEG/WebP inputs and erases useful format fidelity.

## Code Boundaries

### `crates/remote-exec-daemon/src/image.rs`

- add explicit processing branches for:
  - default no-resize passthrough
  - default resize-and-reencode
  - explicit `"original"`
- add path-context error wrapping for missing-file and processing failures
- keep the existing RPC surface unchanged

### `crates/remote-exec-daemon/tests/image_rpc.rs`

- add focused coverage for:
  - small-image passthrough in default mode
  - resize path for oversized images
  - GIF non-passthrough expectations
  - missing-file, directory, and invalid-image error wording

### `crates/remote-exec-broker/src/tools/image.rs`

- remove or align broker-side invalid-detail prevalidation so visible error text matches the intended wording
- keep current MCP wrapping behavior unchanged otherwise

### `crates/remote-exec-broker/tests/mcp_assets.rs`

- add broker-facing checks for:
  - successful image content plus structured content still working after daemon behavior changes
  - invalid detail and other failures surfacing the expected text-only failure shape

### `crates/remote-exec-proto/src/public.rs`

- no schema change required in this batch
- preserve the current repo-local structured result shape with `target`

### `crates/remote-exec-proto/src/rpc.rs`

- no RPC contract change required in this batch

## Behavior Details

### Default mode

For omitted or `null` detail:

- resolve the file relative to effective cwd
- validate existence and file-ness
- read bytes
- identify the input format
- if dimensions already fit within `2048x768`:
  - return original bytes directly for PNG, JPEG, or WebP
  - process and re-encode GIF and other non-passthrough formats
- if dimensions exceed bounds:
  - resize to fit using `FilterType::Triangle`
  - re-encode according to the chosen output format rules

### `"original"` mode

For `"original"` detail:

- preserve original resolution
- return original bytes for supported passthrough formats
- do not silently substitute resized behavior in this repo-local batch, because model-capability gating is out of scope here

### Error wording

The daemon should be the primary source of error wording for image-specific failures.

That means:

- missing-file and directory checks should construct user-facing messages using the resolved absolute path
- decode and encode failures should be wrapped with `unable to process image at ...`
- invalid `detail` should surface the full daemon-level wording instead of the current shortened broker wording

## Testing Plan

### Daemon tests

Add or extend tests for:

- small PNG default mode preserves the original bytes exactly
- small JPEG default mode preserves the original bytes exactly
- small WebP default mode preserves the original bytes exactly
- oversized image default mode still resizes to fit
- default-mode GIF does not blindly return the original bytes
- invalid `detail` returns the full expected message
- missing-file error includes the absolute path context
- directory path error includes the absolute path context
- invalid-image error includes the absolute path context

### Broker tests

Add or extend tests for:

- success still returns one `input_image` content item plus structured content
- invalid `detail` failure text matches the full Codex-style wording
- failure cases still produce text-only error output, not image content

## Success Criteria

This batch is complete when:

- already-small PNG/JPEG/WebP images in default mode reuse original bytes
- oversized images still resize to fit within `2048x768`
- GIF is not treated as a passthrough-preserved format contract
- visible error strings match the intended path-context and invalid-detail wording
- broker success and failure output shapes remain unchanged
- model-capability gating and model-facing `input_image.detail` remain unchanged and explicitly out of scope
