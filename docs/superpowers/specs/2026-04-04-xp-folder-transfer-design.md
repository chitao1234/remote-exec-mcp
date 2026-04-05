# Windows XP Folder Transfer Design

## Summary

Add directory transfer support to the standalone Windows XP daemon while preserving the existing public `transfer_files` contract and cross-target interoperability with the Rust broker and Unix daemon.

The XP daemon will continue to use plain HTTP and the existing `/v1/transfer/export` and `/v1/transfer/import` endpoints. For `source_type=file`, the HTTP body is a GNU tar stream containing exactly one regular file entry at `.remote-exec-file`, matching the Rust daemon and broker-local staging contract. For `source_type=directory`, the HTTP body is a GNU tar stream for the directory tree.

## Goals

- Preserve cross-target directory transfer compatibility for `xp -> xp`, `xp -> unix/local`, and `unix/local -> xp`.
- Keep the broker transfer flow generic. No XP-specific broker protocol, RPC, or archive translation layer.
- Match current directory overwrite semantics: the destination path is the root directory to create or replace.
- Preserve empty directories and nested directory trees.
- Keep validation strict and fail closed on malformed or unsupported archive entries.

## Non-Goals

- Generic support for arbitrary tar producers beyond the subset used by this project.
- Support for symlinks, hard links, device entries, sparse files, pax headers, or image transfer on XP.
- Permission or ownership fidelity beyond what is needed for interoperability.
- Introducing TLS or PTY support into the XP daemon.

## External Contract

The public `transfer_files` tool contract remains unchanged.

- Broker export still posts `TransferExportRequest { path }` to `/v1/transfer/export`.
- Broker import still posts the archive body to `/v1/transfer/import` with:
  - `x-remote-exec-destination-path`
  - `x-remote-exec-overwrite`
  - `x-remote-exec-create-parent`
  - `x-remote-exec-source-type`
- `x-remote-exec-source-type=file` means the body is a GNU tar stream containing exactly one regular file entry at `.remote-exec-file`.
- `x-remote-exec-source-type=directory` means the body is a GNU tar stream.

No new headers, no broker feature flags, and no XP-only transport branches are introduced.

## Directory Semantics

Directory transfer on XP follows the same high-level semantics as the Rust daemon.

- Export accepts an absolute source path that must resolve to a directory.
- Import treats `destination.path` as the directory root, not a parent container.
- `overwrite=fail` rejects any existing destination path.
- `overwrite=replace` removes the existing destination path first, whether it is a file or directory.
- `create_parent=false` requires the destination parent directory to already exist.
- `create_parent=true` creates missing destination parent directories before extraction.

Copy accounting follows the existing result shape.

- `source_type=directory`
- `bytes_copied` is the sum of extracted regular file payload bytes
- `files_copied` counts extracted regular files
- `directories_copied` counts the root directory plus extracted directory entries beneath it
- `replaced` reports whether the destination path existed and was removed first

## Archive Format

The XP daemon will implement a narrow GNU tar subset that is sufficient for interoperability with the Rust daemon.

Supported outbound entries:

- GNU long-name records
- Regular file entries
- Directory entries
- End-of-archive zero blocks

Supported inbound entries:

- GNU long-name records
- Regular file entries
- Directory entries
- End-of-archive zero blocks

Rejected inbound entries:

- Symlinks and hard links
- Character, block, fifo, and other special entries
- GNU sparse entries
- Pax headers and other unsupported extension records
- Absolute paths, drive-qualified paths, UNC paths, or traversal outside the destination root

The XP daemon does not need to preserve Unix ownership or mode fidelity. It may emit safe default mode values in tar headers. Long paths must use GNU long-name records because the Rust tar builder emits GNU long-name entries when header paths exceed legacy field limits.

## XP Daemon Changes

All behavior stays inside `crates/remote-exec-daemon-xp` unless tests reveal a broker gap.

### `transfer_ops.*`

Refactor the current file-only transfer helpers into generalized helpers that support:

- file export/import using a single-file GNU tar stream at `.remote-exec-file`
- directory export/import using GNU tar streams

Expected responsibilities:

- absolute path validation for source and destination paths
- destination preparation and overwrite handling
- recursive directory traversal for export
- archive writing for directories, including explicit directory entries
- archive parsing for directory import
- per-entry path validation and normalization
- copy summary accounting

### `server.cpp`

Update transfer routes to delegate both file and directory operations through the generalized transfer helpers.

- `/v1/transfer/export` returns tar bytes for both files and directories.
- `/v1/transfer/import` accepts `source_type=file` and `source_type=directory`.
- Directory import no longer returns the current single-file-only rejection.
- Error handling remains intentionally simple and maps failures into the existing RPC error envelope.

## Validation Rules

### Source and destination paths

- Export source path must be absolute.
- Import destination path must be absolute.
- Export source must be a regular file or directory.
- Import parent existence and creation behavior must continue to honor `create_parent`.

### Archive entry paths

For directory import, every archive member path must satisfy all of the following before writing anything outside the destination root:

- relative path only
- no leading slash or backslash
- no drive prefix such as `C:`
- no UNC prefix
- no empty path after normalization
- no `.` or `..` components that escape or alias the destination root

Archive paths are validated in archive form first and then normalized to Windows separators when materializing them on disk.

### Archive structure

- Header checksum must validate.
- File payload length must match the tar size field.
- Unsupported typeflags fail the import.
- Truncated headers, truncated payloads, or malformed long-name sequences fail the import.
- The importer stops at the standard two zero blocks or end-of-buffer once a valid terminal condition is reached.
- File import requires exactly one regular file entry at `.remote-exec-file` and rejects any extra archive entries.

## Testing

### XP daemon unit tests

Extend `crates/remote-exec-daemon-xp/tests/test_transfer.cpp` to cover:

- single-file tar export/import using `.remote-exec-file`
- directory export and import round trips
- nested directory trees
- empty directory preservation
- overwrite fail vs replace behavior
- `create_parent` behavior
- rejection of traversal paths
- rejection of unsupported typeflags
- long relative path handling through GNU long-name records

### Broker and cross-target verification

Add or extend broker tests so the public `transfer_files` path continues to work with `source_type=file` and `source_type=directory`.

Verification should include real XP daemon execution under Wine for at least:

- `local -> xp` directory transfer through the broker
- `xp -> local` directory transfer through the broker
- negative checks for unsupported archive entries or malformed directory imports

## Risks and Mitigations

### GNU tar compatibility

Risk:
The Rust daemon emits GNU long-name records, so a too-small parser on XP would fail valid cross-target transfers.

Mitigation:
Support GNU long-name records explicitly and add tests that use archive shapes compatible with the Rust daemon.

### Path traversal bugs

Risk:
Directory import is the first XP feature that writes multiple filesystem entries from a remote payload.

Mitigation:
Validate archive member paths before extraction, reject absolute or escaping paths, and keep destination replacement logic separate from per-entry writes.

### Over-scoping into generic tar support

Risk:
Trying to support all tar variants would add code and failure modes unrelated to project needs.

Mitigation:
Limit support to regular files, directories, GNU long-name records, and standard termination blocks. Reject everything else.

## Recommended Approach

Implement a small, narrow GNU tar reader/writer inside the XP daemon rather than changing the broker or vendoring a larger archive stack.

This is the smallest change that:

- preserves the existing public protocol
- keeps broker routing generic
- supports cross-target directory copies
- preserves empty directories
- matches current replace/create-parent semantics closely enough for v1
