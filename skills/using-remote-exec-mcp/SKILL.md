---
name: using-remote-exec-mcp
description: Use when work must happen through a remote-exec-mcp broker on a named target or broker-host `local`, including target discovery with `list_targets`, remote command execution with `exec_command`, live session continuation with `write_stdin`, remote edits with `apply_patch`, image inspection with `view_image`, cross-endpoint copies with `transfer_files`, and TCP/UDP forwarding with `forward_ports`
---

# Using remote-exec-mcp

This skill is self-contained. An agent should be able to use the public `remote-exec-mcp` tools directly from this file without reading repository code or `README.md`.

## What This Toolset Is

`remote-exec-mcp` is a brokered remote-work toolset. It gives you seven public tools:

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`
- `forward_ports`

Use it when work belongs on a named remote machine, on the broker host exposed as `target: "local"`, when files must move between endpoints, or when network ports must be forwarded between endpoints.

Do not use this skill for ordinary work in your current Codex workspace. `remote-exec-mcp` is for broker-managed endpoints, not for your current local shell unless the user explicitly wants the broker host.

## Mental Model

- Agents talk to one broker.
- The broker owns the public `session_id` namespace.
- The broker owns the public `forward_id` namespace for `forward_ports`.
- The daemon on each machine owns its own private local session identifiers.
- `session_id` is opaque. Treat it as a broker token, not as a PID and not as a daemon session id.
- `forward_id` is opaque and broker-scoped. Treat it as a broker token, not as daemon-local state, and do not expect it to survive a broker restart.
- Every machine-local operation is target-scoped. Pick the right `target` first.
- `local` means the broker host, not your current Codex workspace.
- For `exec_command`, `write_stdin`, `apply_patch`, and `view_image`, `local` exists only when the broker-host local target is enabled.
- For `transfer_files`, `local` always means the broker-host filesystem endpoint and may be usable even when it does not appear in `list_targets`.
- For `forward_ports`, side `"local"` always means the broker-host network endpoint and may be usable even when it does not appear in `list_targets`.
- Choosing a `target` grants broad access on that machine, optionally narrowed only by static sandbox config for the relevant path-based operation.
- One command call runs on one endpoint only. If bytes must cross endpoints, use `transfer_files`.
- A port forward has a `listen_side` and a `connect_side`; swap them to reverse direction.
- `list_targets` is inventory, not a live health check. A target with `daemon_info: null` may still be configured and valid.
- A broker may be configured with `disable_structured_content = true`. When that is enabled, successful structured-result tool calls omit structured content and you must rely on normal text or image content instead. `apply_patch` is text-only either way.

## When To Use Which Tool

- Need to discover valid target names or PTY capability: `list_targets`
- Need to run a command on one endpoint: `exec_command`
- Need to continue or poll a still-running command: `write_stdin`
- Need to edit files on one endpoint with patch text: `apply_patch`
- Need to inspect an image file that already exists on one endpoint: `view_image`
- Need to move a file or directory tree between endpoints: `transfer_files`
- Need to expose or tunnel TCP/UDP ports between endpoints: `forward_ports`

## First Moves

1. Start with `list_targets({})` unless the target name is already known and trustworthy.
2. Decide whether the task is a `command on one endpoint`, `edit on one endpoint`, `image read on one endpoint`, or `copy between endpoints`.
3. If the task crosses endpoint boundaries, use `transfer_files`.
4. If the task stays on one endpoint, use `exec_command`, `apply_patch`, or `view_image`.
5. If `exec_command` returns a `session_id`, keep it and use `write_stdin` for follow-up input or polling.

## Tool Contracts

The structured result shapes below apply when the broker leaves structured content enabled. `apply_patch` is the exception and remains text-only.

### `list_targets`

Use this to discover valid target names and cached daemon metadata.

Input:

```json
{}
```

Structured result shape:

```json
{
  "targets": [
    {
      "name": "builder-a",
      "daemon_info": {
        "daemon_version": "0.1.0",
        "hostname": "builder-a-host",
        "platform": "linux",
        "arch": "x86_64",
        "supports_pty": true
      }
    },
    {
      "name": "builder-b",
      "daemon_info": null
    }
  ]
}
```

Use the result this way:

- Reuse `targets[].name` exactly in later tool calls.
- Read `daemon_info.platform` to choose endpoint-native absolute paths for `transfer_files`.
- Read `daemon_info.supports_pty` before using `tty: true`.
- If `daemon_info` is `null`, do not invent your own meaning. It only means the broker has no current cached metadata.
- If broker-host exec support is enabled, `local` appears here as a normal target entry.

### `exec_command`

Use this to run a command on one target.

Input shape:

```json
{
  "target": "builder-a",
  "cmd": "pwd",
  "workdir": "/srv/app",
  "shell": "/bin/sh",
  "tty": false,
  "yield_time_ms": 1000,
  "max_output_tokens": 4000,
  "login": true
}
```

Required fields:

- `target`
- `cmd`

Optional fields:

- `workdir`
- `shell`
- `tty`
- `yield_time_ms`
- `max_output_tokens`
- `login`

How to use it:

- Use it for inspection, builds, tests, shell utilities, and one-shot scripts on exactly one endpoint.
- Set `workdir` intentionally. Do not assume the daemon default is the directory you want.
- Use `tty: true` when the program is interactive or when you expect to send more input later.
- Keep the returned `session_id` when present. That means the command is still running.
- Read `session_command` if you need the original command string echoed back.
- Read `original_token_count` to understand whether output was truncated by `max_output_tokens`.
- Read `warnings` when present. Warnings are also surfaced in the normal text output.
- Do not send patch text through `exec_command`. The broker may intercept obvious `apply_patch` shell wrappers for compatibility, but that path emits a warning and is not the preferred workflow.

Common example, one-shot command:

```json
{
  "target": "builder-a",
  "cmd": "rg -n \"TODO|FIXME\" src",
  "workdir": "/srv/project",
  "yield_time_ms": 1000,
  "max_output_tokens": 4000
}
```

Common example, long-running interactive session:

```json
{
  "target": "builder-a",
  "cmd": "python3",
  "workdir": "/srv/project",
  "tty": true,
  "yield_time_ms": 250
}
```

Structured result fields that usually matter:

- `target`
- `exit_code`
- `session_id`
- `session_command`
- `output`
- `original_token_count`
- `warnings`

Interpretation:

- `session_id: null` means the command has already completed.
- `session_id: "..."` means the command is still running and can be continued or polled with `write_stdin`.
- `warnings` appears only when the broker or daemon needs to surface non-fatal warnings. The same warning text is also included in the normal text output.

### `write_stdin`

Use this only with a live `session_id` returned by `exec_command`.

Input shape:

```json
{
  "session_id": "sess_123",
  "chars": "help\n",
  "yield_time_ms": 250,
  "max_output_tokens": 4000,
  "target": "builder-a"
}
```

Required fields:

- `session_id`

Optional fields:

- `chars`
- `yield_time_ms`
- `max_output_tokens`
- `target`

How to use it:

- Send `chars` when you need to answer a prompt or continue an interactive program.
- Send `chars: ""` or omit `chars` when you only need to poll for more output.
- You may omit `target`. The broker can route by `session_id` alone.
- If you provide `target`, it must match the original session target or the call fails.
- Reuse the returned `session_id` until it becomes `null`.
- Read `warnings` when present. When structured content is enabled, `write_stdin` returns the same structured exec result shape as `exec_command`.

Important failure cases:

- If the session is gone, the broker returns `write_stdin failed: Unknown process id <session_id>`.
- If the daemon restarted, the broker also normalizes that into the same `Unknown process id ...` message.
- If stdin was closed for the original session, rerun the program with `exec_command(..., "tty": true)` instead of trying to salvage the old session.

Typical polling call:

```json
{
  "session_id": "sess_123",
  "chars": "",
  "yield_time_ms": 1000
}
```

### `apply_patch`

Use this for direct file edits on exactly one target.

Input shape:

```json
{
  "target": "builder-a",
  "workdir": "/srv/project",
  "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-fn old_name() {}\n+fn new_name() {}\n*** End Patch\n"
}
```

Required fields:

- `target`
- `input`

Optional fields:

- `workdir`

How to use it:

- Prefer `apply_patch` over shell editing when you know the exact file changes.
- Use the same patch discipline as the normal Codex `apply_patch` tool.
- Relative file paths in the patch are resolved from `workdir` when provided.
- C++ daemon targets support both absolute patch paths and paths relative to `workdir`.
- The patch engine supports the documented `*** End of File` marker.
- Updating an existing file preserves its current `LF` versus `CRLF` line ending style.
- This tool is target-local. It does not move bytes between endpoints.

Result behavior:

- Human text reports the updated files.
- Successful calls do not include structured content. Read the normal text output for the patch summary.

### `view_image`

Use this when an image already exists on the endpoint you are working on.

Input shape:

```json
{
  "target": "builder-a",
  "path": "/srv/project/chart.png",
  "workdir": "/srv/project",
  "detail": "original"
}
```

Required fields:

- `target`
- `path`

Optional fields:

- `workdir`
- `detail`

How to use it:

- Use `path` for the image file on that target.
- Use `workdir` only when the path is relative and you need predictable resolution.
- Omit `detail` for the target default behavior.
- On Rust daemon targets, omitted `detail` keeps the default resized-preview behavior.
- On C++ daemon targets, omitted `detail` defaults to `original` because only passthrough image formats are supported there.
- Use `detail: "original"` when full-fidelity inspection matters or when you want to force original bytes on Rust daemon targets.
- The only supported non-empty `detail` value is `original`.
- Errors for unsupported detail values are explicit. Example message:
  `view_image.detail only supports original; omit detail for default ...`

Result behavior:

- The tool returns image content as an MCP `input_image`.
- Structured content includes:

```json
{
  "target": "builder-a",
  "image_url": "data:image/png;base64,...",
  "detail": "original"
}
```

Platform notes:

- Some targets may not implement image reads.
- `remote-exec-daemon-cpp` supports passthrough `view_image` reads for PNG, JPEG, WebP, and GIF only.

### `transfer_files`

Use this whenever bytes must cross endpoint boundaries.

Input shape:

```json
{
  "sources": [
    {
      "target": "local",
      "path": "/tmp/report.txt"
    },
    {
      "target": "local",
      "path": "/tmp/screenshots"
    }
  ],
  "destination": {
    "target": "builder-a",
    "path": "/srv/inbox"
  },
  "exclude": [
    "**/*.log",
    ".git/**"
  ],
  "overwrite": "merge",
  "destination_mode": "auto",
  "symlink_mode": "preserve",
  "create_parent": true
}
```

Required fields:

- exactly one of `source` or `sources`
- `destination.target`
- `destination.path`
- `create_parent`

Supported `overwrite` values:

- `fail`
- `merge` (default)
- `replace`

Supported `destination_mode` values:

- `auto` (default)
- `exact`
- `into_directory`

Supported `symlink_mode` values:

- `preserve` (default)
- `follow`
- `skip`

How to use it:

- Use it for `local -> remote`, `remote -> local`, `remote -> remote`, and `local -> local`.
- Provide either one `source` object or a `sources` array, never both.
- Every source path and the destination path must be absolute for their own endpoint platform.
- `destination_mode: "auto"` gives single-source transfers `cp`-like behavior: copy beneath `destination.path` when it is an existing directory or ends in a path separator, otherwise use it as the exact final path. Multi-source transfers treat `destination.path` as a directory root and place each source beneath it using that source's basename.
- Use `destination_mode: "into_directory"` to copy each source beneath `destination.path` using the source basename.
- `overwrite: "merge"` overlays files into an existing compatible destination without deleting unrelated directory entries; use `replace` only when deleting the existing destination first is intended.
- Unsupported special entries inside source directory trees such as device nodes, FIFOs, and sockets are skipped and returned as warnings.
- `exclude` is optional and is matched relative to each source root during export, not against the destination path.
- `exclude` patterns use `/` as the logical separator on every platform and support `*`, `?`, `**`, `[abc]`, `[a-z]`, `[!abc]`, `[!a-c]`, `[^abc]`, and `[^a-c]`.
- Matching `exclude` entries are omitted silently. Excluded directories are pruned recursively. In v1, single-file sources ignore `exclude`.
- `symlink_mode: "preserve"` copies symlinks as symlinks. Use `follow` to copy symlink targets or `skip` to omit symlinks with warnings. On Windows XP-compatible C++ daemon targets, symlink entries inside directory transfers and import archives are skipped with warnings when preservation is unavailable, while `follow` copies regular-file and directory targets when the platform exposes them.
- `create_parent: true` only creates missing parents for the exact final destination path or destination root you requested.
- If a source target and the destination target are the same, those two paths still must differ.
- Use endpoint-native absolute paths. Linux endpoints use Unix absolute paths such as `/srv/app/file.txt`. Windows endpoints accept drive-qualified paths such as `C:/work/file.txt` and also MSYS/Cygwin-style absolute paths such as `/c/work/file.txt` and `/cygdrive/c/work/file.txt`.
- Transfer compression is an internal broker detail. Do not send a `compression` field.
- The broker automatically uses compressed staging only when its own config and all participating remote targets support it, and otherwise falls back internally.
- Reserved transfer archive summary entries such as `.remote-exec-transfer-summary.json` are internal and are not copied to the destination.
- Reach for `transfer_files` instead of `scp`, `cp`, shell redirection, or ad hoc archives whenever data must move between endpoints.

Structured result fields:

- `source`
- `sources`
- `destination`
- `resolved_destination`
- `destination_mode`
- `source_type`
- `bytes_copied`
- `files_copied`
- `directories_copied`
- `replaced`
- `warnings`

Result interpretation:

- `sources` is always present.
- `source` is only populated for single-source compatibility.
- `source_type` can be `file`, `directory`, or `multiple`.
- `warnings` is present when transfer handling skips source entries or symlinks. The same warning text is also included in normal text output.

Example: download a remote log to broker-host `local`:

```json
{
  "source": {
    "target": "builder-a",
    "path": "/srv/project/build.log"
  },
  "destination": {
    "target": "local",
    "path": "/tmp/build.log"
  },
  "overwrite": "replace",
  "create_parent": true
}
```

### `forward_ports`

Use this whenever TCP or UDP traffic must cross endpoint boundaries.

Open input shape:

```json
{
  "action": "open",
  "listen_side": "local",
  "connect_side": "builder-a",
  "forwards": [
    {
      "listen_endpoint": "127.0.0.1:5432",
      "connect_endpoint": "127.0.0.1:5432",
      "protocol": "tcp"
    }
  ]
}
```

List input shape:

```json
{
  "action": "list",
  "forward_ids": ["fwd_..."]
}
```

Close input shape:

```json
{
  "action": "close",
  "forward_ids": ["fwd_..."]
}
```

How to use it:

- `listen_side` is where the listening socket is opened.
- `connect_side` is where outbound connections or datagrams are sent.
- Either side may be a configured target or `"local"`.
- Bare endpoint strings such as `"8080"` mean `"127.0.0.1:8080"`.
- `listen_endpoint` may use port `0`; read the structured result's `listen_endpoint` for the actual bound port.
- `connect_endpoint` must use a nonzero port.
- Non-loopback bind addresses such as `"0.0.0.0:8080"` are allowed.
- Supported `protocol` values are `tcp` and `udp`.
- Keep the returned `forward_id`; use it to close the forward explicitly.
- `forward_id` is broker runtime state only. If the broker restarts, reopen the forward instead of trying to reuse the old id.
- If only the broker-daemon transport drops while the daemon stays alive, the broker may resume the same forward and preserve future listen-side TCP accepts or UDP datagrams.
- Do not expect active TCP streams or UDP per-peer connector state to survive a reconnect; treat those as lost and let future connections recreate them.
- If the daemon restarts, or if the broker restarts and loses the broker-owned mapping, treat that forward as gone and open a new one.
- If the broker crashes without reconnecting, daemon-side listeners are reclaimed after the internal reconnect grace window expires, and reopening still creates a fresh `forward_id`.

Structured result fields:

- `action`
- `forwards[].forward_id`
- `forwards[].listen_side`
- `forwards[].listen_endpoint`
- `forwards[].connect_side`
- `forwards[].connect_endpoint`
- `forwards[].protocol`
- `forwards[].status`
- `forwards[].last_error`

## Standard Workflows

### Inspect And Edit Remote Code

1. Call `list_targets`.
2. Use `exec_command` to inspect files, search the tree, or run status commands on the chosen target.
3. Use `apply_patch` to edit the file on that same target.
4. Use `exec_command` again for verification, tests, or formatting on that target.

### Upload, Run, Retrieve

1. Use `transfer_files` to copy local input to the target.
2. Use `exec_command` on the target to run the program or script.
3. Use `transfer_files` to bring logs or artifacts back to `local` if needed.

### Continue An Interactive Program

1. Check `supports_pty` from `list_targets` if a real TTY matters.
2. Start the program with `exec_command` and `tty: true`.
3. Keep the returned `session_id`.
4. Use `write_stdin` to send input or poll until the session ends.

### Inspect A Remote Image

1. Use `exec_command` first if you need help locating the image path.
2. Use `view_image` on that target and path.

### Move Content Between Two Remote Targets

1. Use `transfer_files` with one remote source target and a different remote destination target.
2. Use `exec_command` on the destination target if you need to verify the copied content.

## Platform And Compatibility Notes

- PTY support is target-specific. Trust `list_targets().targets[].daemon_info.supports_pty`, not assumptions.
- `remote-exec-daemon-cpp` is narrower than the main daemon.
- On POSIX C++ daemon targets, `tty: true` works when `supports_pty` is true.
- On Windows XP-compatible C++ daemon targets, `tty: true` is rejected.
- On C++ daemon targets, `view_image` supports only passthrough PNG, JPEG, and WebP reads, and omitted `detail` defaults to `original`.
- On POSIX C++ daemon targets, shell selection follows the Rust daemon policy and child processes force `LC_ALL=C.UTF-8` plus `LANG=C.UTF-8`.
- On POSIX C++ daemon targets, non-PTY exec intentionally starts with stdin closed; use `tty: true` when later `write_stdin` input is needed.
- On Windows XP-compatible C++ daemon targets, non-PTY exec intentionally keeps stdin open for compatibility with the original XP daemon.
- On Windows XP-compatible C++ daemon targets, the supported shell is `cmd.exe`.
- On C++ daemon targets, `apply_patch` supports both absolute patch paths and paths relative to `workdir`.
- On C++ daemon targets, static path sandboxing can restrict exec cwd, transfer read/write endpoints, and patch write targets.
- On C++ daemon targets, `transfer_files` supports regular files, directory trees, and broker-built multi-source bundles.
- On C++ daemon targets, transfer archive bodies stream through the daemon instead of requiring a full tar archive to be staged in memory.
- On POSIX C++ daemon targets, transfer symlink modes are supported.
- On Windows XP-compatible C++ daemon targets, symlink entries inside directory transfers and import archives are skipped with warnings when preservation is unavailable; use `symlink_mode: "follow"` to copy regular-file and directory targets when the platform exposes them.
- On C++ daemon targets, transfer compression is never used; the broker falls back automatically.
- On C++ daemon targets, `forward_ports` uses the same daemon-private HTTP/1.1 Upgrade tunnel, reconnect behavior, and broker-owned `forward_id` lifecycle as the Rust daemon.
- On C++ daemon targets, recoverable peer abort/reset errors during forwarding surface as normal tool errors and do not terminate the daemon.
- Do not assume hard links, sparse files, or special files transfer on C++ daemon targets; special files are skipped during export.

## Common Mistakes

- Guessing target names instead of calling `list_targets`.
- Forgetting that `local` means the broker host, not the current Codex workspace.
- Running a command on one target and expecting it to read or write files on another target.
- Using shell tricks for cross-endpoint copy instead of `transfer_files`.
- Treating `destination.path` as if it had the same meaning for both single-source and multi-source transfers.
- Expecting `overwrite: "merge"` to delete files that are absent from the source.
- Sending non-absolute paths to `transfer_files`.
- Sending an unsupported `compression` field to `transfer_files`.
- Leaving `forward_ports` forwards open after they are no longer needed.
- Reversing `listen_side` and `connect_side` when opening a port forward.
- Reusing a stale `forward_id` after a broker or daemon restart instead of reopening the forward.
- Starting an interactive program without `tty: true` and then expecting writable stdin later.
- Using `write_stdin` after the session has already exited or after a daemon restart.
- Sending patch text through `exec_command` instead of using `apply_patch`.
- Passing `detail: "low"` or any other unsupported value to `view_image`.

## Minimal Decision Rules

- Remote command on one endpoint: `exec_command`
- More input or polling for a live session: `write_stdin`
- Direct file edit on one endpoint: `apply_patch`
- Existing image inspection on one endpoint: `view_image`
- Any copy between endpoints: `transfer_files`
- TCP/UDP forwarding between endpoints: `forward_ports`
- Need a valid target name or PTY capability first: `list_targets`
