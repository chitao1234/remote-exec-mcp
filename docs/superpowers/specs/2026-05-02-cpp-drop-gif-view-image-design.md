# C++ Drop GIF `view_image` Support Design

Status: approved design captured in writing

Date: 2026-05-02

References:

- `crates/remote-exec-daemon-cpp/src/server_routes.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`
- `crates/remote-exec-daemon-cpp/README.md`
- `README.md`

## Goal

Remove GIF input support from the C++ daemon `view_image` path while preserving the existing unsupported-format error surface.

## Scope

This design covers only the following changes:

- stop accepting GIF files in C++ `view_image`
- keep unsupported GIF input on the existing `image_decode_failed` / unsupported-format path
- add route coverage for the rejection
- update docs to remove GIF from the supported C++ image formats list

This design does not cover:

- any Rust daemon image behavior
- any new error code or message
- image resizing or re-encoding changes
- broader image-policy alignment between Rust and C++

## Current Behavior Summary

Today the C++ daemon accepts GIF input in `view_image` by recognizing `GIF87a` and `GIF89a` magic bytes and returning the original file as `data:image/gif;base64,...`.

That behavior is narrower than Rust in implementation style, but broader in accepted passthrough formats.

## Decision Summary

### 1. Remove GIF from the C++ passthrough format sniffing

The C++ image path should continue to support only direct passthrough formats, but that set becomes:

- PNG
- JPEG
- WebP

GIF is removed from the accepted set.

### 2. Reuse the existing unsupported-format failure path

When a GIF file is provided to C++ `view_image`, it should fall through the existing format-sniff logic and raise the same generic unsupported-format error path already used for unsupported image signatures.

That means:

- no new error code
- no GIF-specific message
- no new branching in route-level error mapping

The existing `image_decode_failed` surface remains the contract.

### 3. Keep default `detail` behavior unchanged

This change does not alter the C++ daemon’s `detail` policy:

- omitted `detail` still defaults to `original`
- only `original` is accepted explicitly

Only the accepted image input set changes.

## Code Boundaries

### `crates/remote-exec-daemon-cpp/src/server_routes.cpp`

- remove the GIF magic-byte detection branch from `image_mime_type(...)`
- leave the existing unsupported-format throw path unchanged

### `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`

- add a GIF route regression that now expects failure
- keep PNG pass-through coverage unchanged

### `crates/remote-exec-daemon-cpp/README.md`

- change the supported C++ `view_image` passthrough list from PNG/JPEG/WebP/GIF to PNG/JPEG/WebP

### `README.md`

- update the top-level C++ daemon capability summary to remove GIF from the supported `view_image` list

## Error Handling

- GIF input should continue to surface as `image_decode_failed`.
- The underlying message should continue to indicate unsupported image format.
- No new error classification is introduced for this change.

## Testing Strategy

Use targeted C++ route coverage first:

- `make -C crates/remote-exec-daemon-cpp test-host-server-routes`

Then run the broader C++ host and XP gates:

- `make -C crates/remote-exec-daemon-cpp check-posix`
- `make -C crates/remote-exec-daemon-cpp check-windows-xp`

## Rejected Alternatives

### Keep GIF detection and explicitly reject it with the same error

This is rejected because it adds code without adding user-visible value.

Removing the GIF sniff branch is simpler and preserves the same public error behavior.

### Add a dedicated GIF-specific error code

This is rejected because the requested behavior is to reuse the existing unsupported-format error path.
