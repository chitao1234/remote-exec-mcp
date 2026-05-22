# Hidden File Tools Plan

Date: 2026-05-22

## Goal

Add three opt-in MCP tools for direct text-file work:

- `read(file_path, offset?, limit?)`
- `write(file_path, content)`
- `edit(file_path, old_string, new_string, replace_all?)`

These tools are hidden and disabled by default. They become advertised and
callable only when manually enabled in broker config.

The tools intentionally do not accept `workdir`. Every `file_path` resolves
relative to the target daemon's configured default workdir unless the path is
target-native absolute.

## Non-Goals

- Do not replace `apply_patch`; it remains the structured multi-file patch tool.
- Do not replace `transfer_files`; it remains the binary/tree transfer tool.
- Do not add per-call approval or sandbox escalation.
- Do not add CLI subcommands in the first pass unless explicitly requested.
- Do not create missing parent directories for `write`; keep the first version
  narrow.

## Public Tool Contract

### `read`

Input:

```rust
pub struct ReadInput {
    pub target: String,
    pub file_path: String,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
}
```

Semantics:

- `offset` is a one-based line number. Omitted or `0` means `1`.
- `limit` is a line count. Omitted means broker config
  `default_read_limit_lines`, initially `2000`.
- Reject `limit = 0`.
- Reject `limit > max_read_limit_lines`.
- Return MCP text content only.
- Prefix every returned file line with its one-based line number and `": "`.
- Preserve the displayed line text after the prefix, including original line
  endings.
- Append one final reminder line after the displayed content.
- Use the same text decoding policy as `apply_patch`, `write`, and `edit`:
  UTF-8 by default, with experimental target encoding autodetection when the
  target config enables it.

Reminder forms:

```text
file is empty
offset out of range, file only has X lines
EOF reached, file has N lines
showing lines X-Y of N lines
```

Example:

```text
1: int main() {
2:     printf("hello\n");
3: }

EOF reached, file has 3 lines
```

### `write`

Input:

```rust
pub struct WriteInput {
    pub target: String,
    pub file_path: String,
    pub content: String,
}
```

Semantics:

- Overwrite an existing regular file or create a new regular file.
- Reject existing non-file paths.
- Reject missing parent directories.
- New files are written as UTF-8.
- Existing files use the same target text encoding policy as `apply_patch`:
  when experimental encoding autodetection is enabled, preserve the existing
  file encoding and BOM behavior; otherwise write UTF-8 after validating the
  existing file as text.
- Return MCP text content only.

Success messages:

```text
file created successfully with X lines
file updated successfully with X lines
```

Line count:

- Empty content has `0` lines.
- `abc` has `1` line.
- `abc\n` has `1` line.
- `abc\nxyz` has `2` lines.
- `abc\nxyz\n` has `2` lines.

### `edit`

Input:

```rust
pub struct EditInput {
    pub target: String,
    pub file_path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}
```

Semantics:

- Reject `old_string = ""`.
- Read the existing file using the same target text encoding policy as
  `apply_patch`.
- Reject missing files and non-file paths.
- Count non-overlapping matches.
- Reject zero matches.
- Reject multiple matches unless `replace_all = true`.
- Replace the single match or all matches according to `replace_all`.
- Preserve existing file encoding and BOM behavior when experimental encoding
  autodetection is enabled.
- Return MCP text content only.

Preferred success messages:

```text
file updated successfully with X lines, 1 replacement
file updated successfully with X lines, N replacements
```

## Config Contract

Add broker config:

```toml
[tools.file]
# Hidden/off by default. Enabled tools are advertised and callable.
read = false
write = false
edit = false

# Used by read when caller omits limit.
default_read_limit_lines = 2000

# Safety caps.
max_read_limit_lines = 20000
max_read_bytes = 4194304
```

Defaults:

- `read = false`
- `write = false`
- `edit = false`
- `default_read_limit_lines = 2000`
- `max_read_limit_lines = 20000`
- `max_read_bytes = 4194304`

Validation:

- `default_read_limit_lines > 0`
- `max_read_limit_lines > 0`
- `default_read_limit_lines <= max_read_limit_lines`
- `max_read_bytes > 0`

## Hidden Tool Enforcement

Enforce hidden/off behavior in both broker call paths.

MCP server path:

- Register `read`, `write`, and `edit` in `BrokerServer`.
- In `BrokerServer::new`, remove disabled hidden routes with
  `ToolRouter::remove_route`.
- Disabled tools must not appear in `tools/list`.
- Disabled tools must not be callable over MCP.

Direct broker client path:

- `client.rs` bypasses the MCP router and dispatches through `BrokerTool`.
- Add the same hidden-tool enablement check there.
- Disabled hidden tools should behave like unknown tools, not as known disabled
  tools.

## Target and Local Semantics

These tools follow `apply_patch` and `view_image` semantics:

- `target` is required.
- `[local]` enables `target = "local"` for `read`, `write`, and `edit`.
- If `[local]` is omitted, `target = "local"` is unknown for these tools.
- `host_sandbox` applies through the embedded local target.
- `transfer_files` remains the only file tool that can use broker-host
  filesystem access with `target = "local"` when `[local]` is omitted.

## Sandbox Semantics

Path resolution:

- Resolve `file_path` against `state.config.default_workdir`.
- Respect target platform path rules and `windows_posix_root`.
- Lexically normalize before sandbox checks.

Access:

- `read`: `SandboxAccess::Read`
- `write`: `SandboxAccess::Write`
- `edit`: `SandboxAccess::Write`

`edit` should not echo file content, surrounding context, `old_string`, or
`new_string` in errors or logs. It may report match counts.

## Internal RPC Contract

Add daemon-private RPC payloads under `remote-exec-proto/src/rpc/file.rs`.

```rust
pub struct FileReadRequest {
    pub path: String,
    pub offset: Option<u64>,
    pub limit: u64,
    pub max_bytes: u64,
}

pub struct FileReadResponse {
    pub output: String,
    pub lines_returned: u64,
    pub total_lines: u64,
    pub eof: bool,
}

pub struct FileWriteRequest {
    pub path: String,
    pub content: String,
}

pub struct FileWriteResponse {
    pub created: bool,
    pub line_count: u64,
}

pub struct FileEditRequest {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

pub struct FileEditResponse {
    pub replacements: u64,
    pub line_count: u64,
}
```

The broker formats final MCP text messages from these structured responses.

## Capability Contract

Add a daemon capability:

```rust
pub file_tool_protocol_version: Option<FileToolProtocolVersion>
```

Version `1` means the daemon supports:

- `/v1/file/read`
- `/v1/file/write`
- `/v1/file/edit`
- the line-numbered read contract
- write/edit encoding preservation policy

Broker behavior:

- Verify target identity before calling a file tool.
- Check target info capability.
- If missing or lower than `1`, return a clear tool error:

```text
target `NAME` does not support file tool protocol version 1
```

## Error Codes

Add file-specific RPC error codes:

- `file_missing`
- `file_not_file`
- `file_decode_failed`
- `file_too_large`
- `file_old_string_not_found`
- `file_old_string_ambiguous`
- `file_read_failed`
- `file_write_failed`

Reuse existing codes:

- `bad_request` for invalid limit and empty `old_string`
- `sandbox_denied` for sandbox failures
- `internal_error` for unexpected failures

## Rust Implementation Plan

1. Add public file tool schemas in `remote-exec-proto/src/public/file.rs`.
2. Add internal file RPC schemas in `remote-exec-proto/src/rpc/file.rs`.
3. Add `FileToolProtocolVersion` to target capabilities.
4. Add broker config `tools.file`.
5. Add `BrokerTool::{Read, Write, Edit}`.
6. Add hidden-tool route removal in `BrokerServer::new`.
7. Add direct-client hidden-tool gating in `client.rs`.
8. Move `patch/text_codec.rs` into a shared host text codec module.
9. Update patch preflight to use the shared text codec.
10. Add `remote-exec-host/src/file.rs` implementing:
    - default-workdir path resolution
    - sandbox checks
    - line-numbered read output
    - write create/update handling
    - edit replace behavior
    - shared encoding autodetection policy
11. Add Rust daemon file routes:
    - `POST /v1/file/read`
    - `POST /v1/file/write`
    - `POST /v1/file/edit`
12. Add daemon client and local target backend methods.
13. Add broker handlers in `crates/remote-exec-broker/src/tools/file.rs`.
14. Update README, `configs/broker.example.toml`, and
    `skills/using-remote-exec-mcp/SKILL.md`.

## C++ Implementation Plan

If enabled file tools must work on C++ daemon targets, keep C++ aligned with
Rust daemon v1:

1. Add route IDs and route paths for:
   - `/v1/file/read`
   - `/v1/file/write`
   - `/v1/file/edit`
2. Add file tool capability version to target info.
3. Reuse existing default workdir, path normalization, and sandbox helpers.
4. Implement binary-safe file read/write primitives.
5. Validate/decode text as UTF-8 for the first pass unless equivalent encoding
   autodetection is added to C++.
6. Return the same RPC errors and success payloads.
7. Update GNU make, BSD make, and NMAKE source lists.

If C++ cannot support the same encoding autodetection policy immediately, do
not advertise `file_tool_protocol_version = 1` from C++ until it can satisfy the
contract.

## Tests

Broker config and routing:

- Default config does not list `read`, `write`, or `edit`.
- Disabled hidden tools cannot be called over MCP.
- Disabled hidden tools cannot be called through direct `--broker-config`
  dispatch.
- Enabling only one tool lists and calls only that tool.
- `read` is advertised read-only when enabled.

Broker/Rust daemon behavior:

- `read` default limit is `2000` lines.
- `read` uses one-based offset and one-based displayed line numbers.
- `read` treats `offset = 0` as `offset = 1`.
- `read` emits `file is empty`.
- `read` emits `offset out of range, file only has X lines`.
- `read` emits `EOF reached, file has N lines`.
- `read` emits `showing lines X-Y of N lines`.
- `read` enforces max line and byte limits.
- `read` uses the same experimental encoding autodetection policy as
  `apply_patch`.
- `write` creates a file and reports line count.
- `write` updates a file and reports line count.
- `write` rejects missing parent directory.
- `write` rejects non-file paths.
- `write` preserves existing detected encoding when enabled.
- `edit` rejects empty `old_string`.
- `edit` rejects zero matches.
- `edit` rejects multiple matches without `replace_all`.
- `edit` replaces all matches with `replace_all`.
- `edit` preserves existing detected encoding when enabled.
- sandbox read/write denial is enforced.

C++ daemon behavior, if v1 is advertised:

- route smoke tests for read/write/edit.
- line-numbered read reminders.
- duplicate edit match behavior.
- sandbox denial.
- GNU make, BSD make, and NMAKE checks include new files.

## Suggested Commit Slices

1. Config and hidden-tool router/direct-dispatch gating.
2. Proto schemas, capability, and target backend plumbing.
3. Shared host text codec extraction.
4. Rust host and Rust daemon file operations.
5. Broker file tool handlers and tests.
6. C++ daemon v1 support, or explicit no-capability behavior.
7. Docs, config example, and skill updates.
