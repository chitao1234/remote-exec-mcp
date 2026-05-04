# Transfer Files Exclude Design

Status: approved design captured in writing

Date: 2026-05-02

References:

- `README.md`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`
- `crates/remote-exec-broker/src/tools/transfer.rs`
- `crates/remote-exec-broker/src/tools/transfer/operations.rs`
- `crates/remote-exec-broker/src/local_transfer.rs`
- `crates/remote-exec-daemon/src/transfer/mod.rs`
- `crates/remote-exec-daemon/src/transfer/archive/export.rs`
- `crates/remote-exec-daemon-cpp/include/transfer_ops.h`
- `crates/remote-exec-daemon-cpp/src/server.cpp`
- `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`
- `crates/remote-exec-broker/tests/mcp_transfer.rs`
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`

## Summary

Add an `exclude` parameter to the public `transfer_files` tool so callers can omit files and directories during export by using source-root-relative glob patterns.

The broker remains a thin router. Exclusion is enforced only by the exporter that owns the source filesystem:

- broker-host `local` exports through the Rust transfer archive code
- Rust daemon targets export through the Rust daemon archive code
- C++ daemon targets export through the standalone C++ export walker

Import behavior and transfer result shapes remain unchanged in v1.

## Goals

- Let callers exclude source-root-relative files and directories during `transfer_files`.
- Preserve one consistent public contract across broker-host `local`, Rust daemon targets, and C++ daemon targets.
- Prune excluded directory subtrees before they are archived or streamed.
- Keep multi-source transfer behavior symmetric by applying the same exclude list independently to each source root.
- Preserve the current broker-mediated transfer architecture and streaming behavior.

## Non-Goals

- Sync, mirror, or include-and-reinclude semantics.
- Whole-pattern negation similar to gitignore `!pattern`.
- Broker-side archive rewriting or broker-side path filtering.
- Import-side exclusion.
- Adding exclusion outcome fields to transfer results.
- Excluding the top-level source item itself in v1.

## Current Behavior Summary

Today `transfer_files` accepts source selection, destination, overwrite mode, destination mode, symlink mode, and `create_parent`. The public input does not expose exclusion patterns.

The broker turns each source into a `TransferExportRequest` that currently includes only:

- `path`
- `compression`
- `symlink_mode`

The exporter on each host then walks the source tree and writes a GNU tar stream:

- the Rust daemon handles transfer export in `crates/remote-exec-daemon/src/transfer/archive/export.rs`
- broker-host `local` reuses that same Rust archive implementation via `crates/remote-exec-broker/src/local_transfer.rs`
- the C++ daemon handles transfer export in `crates/remote-exec-daemon-cpp/src/transfer_ops_export.cpp`

That means exclusions belong in the export request and must be enforced during export traversal.

## Decision Summary

### 1. Add one public `exclude` field and one matching export RPC field

Add `exclude: Vec<String>` with `#[serde(default)]` to:

- `TransferFilesInput`
- `TransferExportRequest`

The broker copies the public `exclude` list into every per-source export request. Import RPC remains unchanged.

### 2. Match relative to each source root

Patterns are evaluated relative to the root of each source entry, not against absolute host paths.

Examples:

- source `/repo` with `exclude = ["build/**"]` excludes `/repo/build/...`
- source `C:/work/app` with `exclude = ["**/*.log"]` excludes `C:/work/app/logs/run.log`

For multi-source transfers, the same `exclude` list is evaluated independently for each source root before any bundling step.

### 3. Enforce excludes only during export

The exporter that owns the filesystem decides what to omit.

The broker does not:

- interpret glob syntax
- filter tar archives after export
- rewrite bundled archives

Importers remain unaware of exclusion rules and simply consume the archive they receive.

### 4. Excluded entries are silent, not warnings

Entries that match an explicit exclude pattern are intentionally omitted and should not produce warnings.

Warnings remain reserved for non-fatal transfer behavior such as:

- skipped unsupported special entries
- skipped symlinks under `symlink_mode = "skip"`

### 5. Excluded directories are pruned recursively

If a directory relative path matches an exclude pattern:

- the directory entry is not archived
- no recursive walk happens below it

This avoids reading or archiving excluded content and preserves the performance benefit of source-side filtering.

### 6. The top-level source item stays included in v1

The exclusion list applies only to descendants beneath a directory source root.

For a single file source:

- `exclude` has no effect in v1

For a directory source:

- the root directory itself still transfers
- if every descendant is excluded, the destination receives an empty directory

This keeps source-type behavior stable and avoids introducing a no-op single-file archive contract in the first version.

## Public API

### Tool input

The public `transfer_files` input gains:

```json
{
  "exclude": [
    ".git/**",
    "**/*.log",
    "build/[a-z]*.tmp"
  ]
}
```

Field semantics:

- `exclude` is optional and defaults to `[]`
- each entry is one glob pattern
- matching is source-root-relative
- `/` is the logical separator for matching on all platforms
- `\` in input patterns is normalized to `/`

The transfer result shape does not change in v1.

### CLI

The broker CLI should expose repeated `--exclude <glob>` flags for `transfer-files`.

## Glob Grammar

Supported syntax:

- `*` matches zero or more characters within one path segment
- `?` matches exactly one character within one path segment
- `**` matches across `/` boundaries
- `[abc]` matches one character from an explicit set
- `[a-z]` matches one character in an explicit range
- `[!abc]` matches one character not in the explicit set
- `[!a-c]` matches one character not in the explicit range
- `[^abc]` matches one character not in the explicit set
- `[^a-c]` matches one character not in the explicit range

Not supported:

- brace expansion such as `{foo,bar}`
- extglob forms such as `!(foo)` or `+(foo)`
- POSIX named character classes such as `[[:alpha:]]`
- whole-pattern negation or re-include semantics

Character class rules:

- `!` or `^` in the first class-body position means negation
- `-` is a range operator only when it appears between two valid class atoms
- malformed classes such as `[` or `[a-` are invalid patterns

## Matching Semantics

### Path normalization

Before matching:

- candidate paths are converted to source-root-relative logical paths
- platform separators are normalized to `/`
- the root directory itself is not matched against exclude patterns

Examples for a source root:

- `src/main.rs`
- `build/output/app.log`
- `.git/config`

### Segment boundaries

`*`, `?`, and character classes do not cross `/`.

Only `**` can match across `/` boundaries.

### Evaluation order

For directory traversal, exclusion matching runs before symlink handling and before unsupported-entry warnings.

That means:

- excluded symlinks are silently omitted instead of producing skip warnings
- excluded special files are silently omitted instead of producing unsupported-entry warnings

### Multi-source behavior

Each source entry gets the same exclude list, but matching is relative to that source’s own root.

This keeps one call portable even when it mixes Linux and Windows source targets.

## Error Handling

Invalid exclude patterns fail the export request before streaming begins.

That means:

- Rust export compilation happens before the archive body is returned
- C++ export compilation happens before the HTTP response headers are sent

Empty patterns should also be rejected as invalid.

The failure should use the existing transfer RPC error envelope. A dedicated new error code is not required in v1 if the current `transfer_failed` or `transfer_source_unsupported` mapping remains adequate, but the error text should be explicit that the exclude pattern is invalid.

## Architecture and Ownership

### Public layer

`crates/remote-exec-proto/src/public.rs` owns the public tool argument shape.

Change:

- add `exclude: Vec<String>` to `TransferFilesInput`

### Broker layer

`crates/remote-exec-broker/src/tools/transfer.rs` and `operations.rs` remain responsible for:

- validating source and destination endpoints
- resolving destination behavior
- choosing local vs remote export/import paths
- copying the public exclude list into per-source export requests

The broker must not own glob parsing or filtering behavior.

### Rust transfer export layer

The Rust daemon export archive code owns:

- compiling exclude patterns once per source export
- matching normalized relative paths during traversal
- pruning directories before recursion

This behavior should live alongside transfer archive export, not in import code and not in the broker.

### C++ transfer export layer

The standalone C++ daemon owns the same behavior for plain HTTP transfer export:

- parse and compile exclude patterns once per export request
- normalize candidate relative paths to `/`
- prune matching directories before recursion

The C++ implementation should follow the same explicit grammar contract as the Rust implementation rather than attempting shell-dependent host globbing.

## Component Changes

### `crates/remote-exec-proto`

Update:

- `src/public.rs`
- `src/rpc.rs`

Changes:

- add `exclude: Vec<String>` to `TransferFilesInput`
- add `exclude: Vec<String>` to `TransferExportRequest`

### `crates/remote-exec-broker`

Update:

- `src/tools/transfer.rs`
- `src/tools/transfer/operations.rs`
- `src/bin/remote_exec.rs`

Changes:

- thread `exclude: &[String]` through single-source and multi-source transfer operations
- include excludes in every `TransferExportRequest`
- add repeated CLI `--exclude` support

No broker-local archive filtering should be added.

### `crates/remote-exec-daemon`

Update:

- `src/transfer/mod.rs`
- `src/transfer/archive/export.rs`
- add a helper module for transfer glob parsing and matching

Changes:

- compile excludes once per export request
- extend prepared export state with the compiled matcher
- consult the matcher before archiving a file or descending into a directory
- ignore excludes for single-file sources in v1

### `crates/remote-exec-daemon-cpp`

Update:

- `include/transfer_ops.h`
- `src/server.cpp`
- `src/transfer_ops_internal.h`
- `src/transfer_ops_export.cpp`
- add a dedicated helper for transfer glob parsing and matching

Changes:

- parse `"exclude"` from `/v1/transfer/export` JSON input
- carry exclude state through `ExportOptions`
- compile excludes before sending streaming response headers
- evaluate normalized relative paths during traversal
- prune matching directories before recursion
- ignore excludes for single-file sources in v1

## Documentation Changes

Update together with the behavior change:

- `README.md`
- `crates/remote-exec-daemon-cpp/README.md`
- `skills/using-remote-exec-mcp/SKILL.md`

Documentation must describe:

- the new `exclude` field
- source-root-relative matching
- supported glob grammar
- the v1 rule that single-file sources ignore excludes
- the fact that excluded entries are silently omitted and not reported as warnings

## Testing

### Broker public-surface tests

Extend `crates/remote-exec-broker/tests/mcp_transfer.rs` to cover:

- local-to-local directory transfer excluding matching files
- pruning of excluded directories
- forwarding of repeated exclude patterns into export requests
- multi-source transfer applying excludes independently per source root
- single-file sources ignoring excludes in v1

If needed, extend stub-daemon capture support in `crates/remote-exec-broker/tests/support/stub_daemon.rs` to assert that the export request body includes the new field.

### Rust daemon transfer tests

Extend `crates/remote-exec-daemon/tests/transfer_rpc.rs` to cover:

- `**/*.log`
- `[abc].txt`
- `[a-z].txt`
- `[!abc].txt`
- `[!a-c].txt`
- `[^abc].txt`
- `[^a-c].txt`
- malformed class rejection
- excluded directories pruned without warnings
- excluded symlinks and excluded unsupported entries omitted silently because they match before warning generation

### C++ daemon tests

Extend:

- `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp`

Coverage should include:

- the same pattern forms as the Rust daemon
- malformed pattern rejection before streaming begins
- route parsing for JSON `exclude`
- subtree pruning behavior
- single-file source behavior remaining unchanged in v1

## Risks and Mitigations

### Cross-daemon semantic drift

Risk:
Rust and C++ matchers could diverge on pattern edge cases.

Mitigation:
Define a deliberately bounded grammar and add matching parity tests in both daemon suites for the same pattern families.

### Overloading warnings

Risk:
Excluded entries could be reported the same way as unsupported entries, which would make results noisy and misleading.

Mitigation:
Keep excluded entries silent and reserve warnings for unrequested non-fatal skips only.

### Broker creep

Risk:
It is tempting to add broker-side archive filtering to avoid touching both daemons.

Mitigation:
Keep exclusion enforcement strictly on the export side, where traversal already happens and where pruning saves IO and streaming work.

### Single-file root ambiguity

Risk:
Allowing exclusion to remove the top-level source file introduces ambiguity about whether a successful transfer can produce no file payload at all.

Mitigation:
Keep single-file source behavior unchanged in v1 and document that excludes only affect descendants beneath directory roots.

## Recommended Approach

Implement exclusion as an export-side capability that flows through the existing public tool and broker-daemon export request shapes.

This is the smallest coherent change because it:

- preserves the current broker-mediated architecture
- keeps matching local to the filesystem owner
- works for broker-host `local`, Rust daemon targets, and C++ daemon targets
- prunes excluded subtrees before archiving
- avoids changing import semantics or transfer result schemas
