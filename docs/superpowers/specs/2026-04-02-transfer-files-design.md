# Transfer Files Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `README.md`
- `docs/local-system-tools.md`
- `docs/specs/2026-03-31-remote-exec-mcp-design.md`
- `crates/remote-exec-broker/src/config.rs`
- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/src/tools/patch.rs`
- `crates/remote-exec-broker/src/tools/targets.rs`
- `crates/remote-exec-daemon/src/server.rs`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`

## Goal

Add one remote-native public tool, `transfer_files`, that covers broker-mediated file and directory transfers across:

- `local -> remote`
- `remote -> local`
- `remote -> remote`

The tool should make cross-machine file movement first-class without exposing staging as a public concept and without turning v1 into a general sync engine.

The symmetric endpoint model also permits `local -> local` transfers on the broker host. That is acceptable in v1, even though it is not the main motivation for the tool.

## Scope

Included:

- one new public tool, `transfer_files`
- exact-path file and directory transfer semantics
- a broker-local filesystem endpoint addressed as `target: "local"`
- broker-mediated transfer for every direction
- explicit overwrite and parent-creation behavior
- broker and daemon RPC additions required to stream file trees through the broker
- public-surface tests and README updates

Excluded:

- sync or mirror semantics
- delete propagation
- multi-source copy
- globbing
- relative path resolution
- public staging or artifact-management tools
- symlink preservation
- ownership, xattr, ACL, or timestamp preservation guarantees

## Current Behavior Summary

Today the workspace exposes:

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`

These tools make remote execution and patching possible, but file transfer is still indirect. An agent can shell out to `cp`, `tar`, `rsync`, or ad hoc scripts, but the public MCP surface does not provide one explicit contract for moving files between the broker host and configured targets.

For remote/local integration, that leaves a real gap:

- agents must invent transport conventions themselves
- remote-to-remote copy requires shell-specific choreography
- directory transfer semantics are implicit rather than tested

## Decision Summary

### 1. Add one public transfer facade, not a larger tool family

V1 adds only one new public tool:

- `transfer_files`

The repo should not add separate `stat_path`, `list_directory`, staging, artifact, upload, or download tools in this batch.

Reasoning:

- inspection already fits under `exec_command`
- public staging would add cognitive overhead for agents
- one facade is enough to prove the transfer model before adding higher-level workflows

### 2. Use `target: "local"` as the broker-host endpoint

Every endpoint is addressed the same way:

```json
{
  "target": "local" | "<configured-target>",
  "path": "/absolute/path"
}
```

Rules:

- `target: "local"` means the broker host filesystem
- any other `target` must be one configured target name
- broker config must reject a remote target named `local`
- the broker remains the only public MCP server; `local` is not a second daemon
- `local -> local` is allowed by the same endpoint model

This keeps the public shape simple and symmetric across all directions.

### 3. Keep the transfer broker-mediated for every direction

Even for `remote -> remote`, the transfer is broker-mediated:

1. read from the source endpoint
2. stream through the broker
3. write to the destination endpoint

V1 does not add direct daemon-to-daemon data paths.

Reasoning:

- simpler trust model
- simpler observability and error handling
- no new cross-target trust relationship between daemons

### 4. Make destination semantics exact, not shell-like

`destination.path` is the exact final path to create or replace.

Examples:

- file `/tmp/a.txt` to `/srv/out/a.txt` writes exactly `/srv/out/a.txt`
- directory `/repo/dist` to `/srv/releases/dist` writes exactly `/srv/releases/dist`

V1 does not support:

- basename inference
- "copy into this existing directory" heuristics
- multiple source paths in one call

This avoids ambiguous `cp`-style behavior that agents often misread.

### 5. Require absolute paths and no `workdir`

Both endpoints must use absolute paths.

The tool does not accept `workdir`.

Reasoning:

- transfer operations are cross-endpoint and should not inherit shell-cwd semantics
- exact absolute paths make routing and error messages more predictable

### 6. Support one regular file or one directory tree as the source root

`source.path` must exist when the call starts and may resolve to:

- one regular file
- one directory tree

The tool rejects:

- a source root that is a symlink
- symlinks encountered inside a copied directory tree
- sockets, FIFOs, block devices, and character devices

Empty directories are allowed and preserved.

Directory copies are recursive.

This keeps the v1 filesystem model intentionally narrow and portable.

### 7. Preserve contents and executability, not full filesystem metadata

V1 guarantees:

- file bytes are preserved
- directory structure is preserved
- executable bit is preserved on regular files

V1 does not guarantee:

- uid or gid preservation
- full mode-bit parity beyond executable-ness
- xattrs
- ACLs
- timestamps

This is enough for the common agent use cases of moving build outputs, scripts, binaries, and small workspace trees without turning the tool into an archive-fidelity project.

### 8. Fail the transfer as a whole on the first unsupported or conflicting condition

`transfer_files` is an all-or-fail tool in v1.

No partial-success mode is exposed.

The call aborts if it encounters:

- an unknown target
- a missing source
- an unsupported source or nested entry type
- a destination conflict under `overwrite: "fail"`
- a missing destination parent under `create_parent: false`
- a read, stream, validation, or write failure

This gives agents a deterministic retry model.

### 9. Reject obvious self-transfer collisions

If `source.target` and `destination.target` are the same and the normalized paths are identical, the call should fail before doing any work.

This avoids accidental destructive no-op or replace-via-self behavior.

## Public API

### Tool name

- `transfer_files`

### Input shape

```json
{
  "source": {
    "target": "builder-a",
    "path": "/repo/dist"
  },
  "destination": {
    "target": "local",
    "path": "/tmp/dist"
  },
  "overwrite": "fail" | "replace",
  "create_parent": true
}
```

### Field semantics

- `source.target`
  - `"local"` for the broker host, otherwise one configured target
- `source.path`
  - absolute source path
- `destination.target`
  - `"local"` for the broker host, otherwise one configured target
- `destination.path`
  - absolute exact final path to create or replace
- `overwrite`
  - `"fail"` rejects if `destination.path` already exists
  - `"replace"` replaces an existing destination path
- `create_parent`
  - `true` creates missing parent directories for `destination.path`
  - `false` requires the parent path to already exist

### Result shape

```json
{
  "source": {
    "target": "builder-a",
    "path": "/repo/dist"
  },
  "destination": {
    "target": "local",
    "path": "/tmp/dist"
  },
  "source_type": "directory",
  "bytes_copied": 12345,
  "files_copied": 12,
  "directories_copied": 3,
  "replaced": false
}
```

Proposed result fields:

- echoed `source`
- echoed `destination`
- `source_type: "file" | "directory"`
- `bytes_copied: u64`
- `files_copied: u64`
- `directories_copied: u64`
- `replaced: bool`

Counter rules:

- `bytes_copied` is the sum of copied regular-file payload bytes
- `files_copied` counts copied regular files
- `directories_copied` counts copied source directories and includes the source root when `source_type` is `directory`

Model-facing text output should stay concise, for example:

```text
Transferred directory `/repo/dist` from `builder-a` to `/tmp/dist` on `local`.
Files: 12, directories: 3, bytes: 12345, replaced: no
```

## Behavioral Semantics

### Source validation

Before streaming begins, the tool validates:

- `source.target` is valid
- `source.path` is absolute
- `destination.target` is valid
- `destination.path` is absolute
- source and destination are not the same normalized endpoint and path
- the source root exists
- the source root is either a regular file or directory

For directory sources, the exported tree must also reject unsupported nested entries such as symlinks or device nodes.

### Destination conflict handling

For `overwrite: "fail"`:

- fail if `destination.path` already exists

For `overwrite: "replace"`:

- allow replacement of an existing file with a file
- allow replacement of an existing directory with a directory
- allow replacement across file-vs-directory boundaries

The replacement semantics are "make `destination.path` become the new source materialized at that exact path", not "merge into existing contents".

### Parent-directory handling

If `create_parent: true`, the implementation creates missing parent directories of `destination.path`.

If `create_parent: false`, the call fails when the parent directory of `destination.path` does not already exist.

### Directory handling

Directory copies are recursive and preserve empty directories.

The destination root is the directory at `destination.path` itself. The tool does not create `destination.path/<basename(source)>`.

### Single-file handling

Single-file transfers create or replace exactly `destination.path`.

The destination is not interpreted as a containing directory, even if it already exists as one under `overwrite: "replace"`.

If `destination.path` exists as a directory under `overwrite: "replace"`, it is replaced as the final destination path rather than merged into.

## Failure and Atomicity Model

### Tool-level failure contract

The tool reports one success result or one error.

It does not expose per-entry partial completion in v1.

### File replace behavior

For single-file transfers, replacement should be atomic when practical:

1. write to a temp file on the destination endpoint
2. fsync when appropriate for the implementation
3. rename into the final path

### Directory replace behavior

For directory transfers, the preferred behavior is:

1. materialize into a temp sibling directory on the destination endpoint
2. rename into the final path

If the implementation cannot perform the replace through a safe temp-and-rename strategy on that destination filesystem, it should fail rather than silently degrading into in-place partial mutation.

V1 therefore aims for strong replace behavior, but does not claim a universal cross-filesystem atomicity guarantee.

## Internal Architecture

The public API stays as one tool, but the internal implementation should use a broker-coordinated archive stream.

High-level flow:

1. broker validates source and destination endpoints
2. source side exports one root file or one root directory tree into a streaming archive representation
3. broker relays the stream
4. destination side validates and materializes it at `destination.path`
5. broker returns counts and byte totals

This internal model is intentionally private. Public callers should not see staging or artifact IDs.

## Broker and Daemon Responsibilities

### Broker

The broker is responsible for:

- validating public tool arguments
- reserving `"local"` as the broker-host pseudo-target
- rejecting configured remote targets named `"local"`
- coordinating source read and destination write
- implementing local read and local write behavior for the broker-host endpoint
- formatting MCP text and structured results

### Daemon

The daemon is responsible for:

- exporting a validated file or directory tree from one absolute source path
- rejecting unsupported filesystem entry types
- importing a streamed file or directory tree to one absolute destination path
- enforcing overwrite and parent-creation rules on the remote endpoint
- preserving executable bits on regular files

This likely requires new internal RPCs for export and import rather than trying to overload the current exec, patch, or image RPCs.

## Documentation and Trust Model Updates

`README.md` should be updated to reflect that:

- `transfer_files` is now part of the public surface
- selecting `target: "local"` is equivalent to full filesystem access on the broker host
- a configured remote target may not be named `local`

This is an intentional expansion of the trust model because the broker host filesystem becomes a first-class transfer endpoint.

## Alternatives Considered

### Separate inspection tools such as `stat_path` and `list_directory`

Rejected for v1 because:

- `exec_command` already covers remote inspection
- they would expand public surface area without closing the core file-movement gap

### Public staging or artifact-management tools

Rejected for v1 because:

- they add workflow complexity for agents
- staging is better treated as an implementation detail if it is needed internally

### Rsync-style sync semantics

Rejected for v1 because:

- delete behavior, merge behavior, and conflict semantics are much harder to specify
- exact transfer semantics are enough to prove the broker-mediated transport model first

## Testing Expectations

The implementation plan should include public-surface coverage for at least:

- `local -> remote` file transfer
- `remote -> local` file transfer
- `remote -> remote` directory transfer
- empty-directory preservation
- executable-bit preservation
- `overwrite: "fail"` conflict rejection before mutation
- `create_parent: false` rejection before mutation
- same-endpoint same-path rejection
- rejection of symlink roots and nested symlinks
- broker config rejection for a remote target named `local`

## Open Questions Deferred Out of Scope

The following are intentionally deferred beyond v1:

- multi-source copy
- include or exclude filters
- symlink-preserving modes
- timestamp preservation
- resumable transfers
- progress events or streaming deltas
- sync or mirror behavior
