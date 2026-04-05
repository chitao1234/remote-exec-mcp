---
name: using-remote-exec-mcp
description: Use when work must happen through a remote-exec-mcp broker on a named target or broker-host `local`, including target discovery with `list_targets`, remote command execution with `exec_command`, live session continuation with `write_stdin`, remote edits with `apply_patch`, image inspection with `view_image`, and cross-endpoint copies with `transfer_files`
---

# Using remote-exec-mcp

This skill is self-contained. An agent should be able to use the public `remote-exec-mcp` tools directly from this file without reading repository code or `README.md`.

## What This Toolset Is

`remote-exec-mcp` is a brokered remote-work toolset. It gives you six public tools:

- `list_targets`
- `exec_command`
- `write_stdin`
- `apply_patch`
- `view_image`
- `transfer_files`

Use it when work belongs on a named remote machine, on the broker host exposed as `target: "local"`, or when files must move between endpoints.

Do not use this skill for ordinary work in your current Codex workspace. `remote-exec-mcp` is for broker-managed endpoints, not for your current local shell unless the user explicitly wants the broker host.

## Mental Model

- Agents talk to one broker.
- The broker owns the public `session_id` namespace.
- The daemon on each machine owns its own private local session identifiers.
- `session_id` is opaque. Treat it as a broker token, not as a PID and not as a daemon session id.
- Every machine-local operation is target-scoped. Pick the right `target` first.
- `local` means the broker host, not your current Codex workspace.
- For `exec_command`, `write_stdin`, `apply_patch`, and `view_image`, `local` exists only when the broker-host local target is enabled.
- For `transfer_files`, `local` always means the broker-host filesystem endpoint and may be usable even when it does not appear in `list_targets`.
- Choosing a `target` is equivalent to full access on that machine for this v1 trust model.
- One command call runs on one endpoint only. If bytes must cross endpoints, use `transfer_files`.
- `list_targets` is inventory, not a live health check. A target with `daemon_info: null` may still be configured and valid.

## When To Use Which Tool

- Need to discover valid target names or PTY capability: `list_targets`
- Need to run a command on one endpoint: `exec_command`
- Need to continue or poll a still-running command: `write_stdin`
- Need to edit files on one endpoint with patch text: `apply_patch`
- Need to inspect an image file that already exists on one endpoint: `view_image`
- Need to move a file or directory tree between endpoints: `transfer_files`

## First Moves

1. Start with `list_targets({})` unless the target name is already known and trustworthy.
2. Decide whether the task is a `command on one endpoint`, `edit on one endpoint`, `image read on one endpoint`, or `copy between endpoints`.
3. If the task crosses endpoint boundaries, use `transfer_files`.
4. If the task stays on one endpoint, use `exec_command`, `apply_patch`, or `view_image`.
5. If `exec_command` returns a `session_id`, keep it and use `write_stdin` for follow-up input or polling.

## Tool Contracts

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

Interpretation:

- `session_id: null` means the command has already completed.
- `session_id: "..."` means the command is still running and can be continued or polled with `write_stdin`.

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
- The patch engine supports the documented `*** End of File` marker.
- This tool is target-local. It does not move bytes between endpoints.

Result behavior:

- Human text reports the updated files.
- Structured content is an empty object: `{}`.

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
- Omit `detail` for default preview behavior.
- Use `detail: "original"` only when full-fidelity inspection matters.
- The only supported non-empty `detail` value is `original`.
- Errors for unsupported detail values are explicit. Example message:
  `view_image.detail only supports original; omit detail for default resized behavior ...`

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
- `remote-exec-daemon-xp` does not support `view_image`.

### `transfer_files`

Use this whenever bytes must cross endpoint boundaries.

Input shape:

```json
{
  "source": {
    "target": "local",
    "path": "/tmp/report.txt"
  },
  "destination": {
    "target": "builder-a",
    "path": "/srv/inbox/report.txt"
  },
  "overwrite": "fail",
  "create_parent": true
}
```

Required fields:

- `source.target`
- `source.path`
- `destination.target`
- `destination.path`
- `overwrite`
- `create_parent`

Supported `overwrite` values:

- `fail`
- `replace`

How to use it:

- Use it for `local -> remote`, `remote -> local`, `remote -> remote`, and `local -> local`.
- Both `source.path` and `destination.path` must be absolute for their own endpoint platform.
- `destination.path` is the exact final path to create or replace.
- `destination.path` does not mean "copy into this directory".
- `create_parent: true` only creates missing parents for that exact final path.
- If `source.target` and `destination.target` are the same, the paths still must differ.
- Use endpoint-native absolute paths. Linux endpoints use Unix absolute paths such as `/srv/app/file.txt`. Windows endpoints accept drive-qualified paths such as `C:/work/file.txt` and also MSYS/Cygwin-style absolute paths such as `/c/work/file.txt` and `/cygdrive/c/work/file.txt`.
- Reach for `transfer_files` instead of `scp`, `cp`, shell redirection, or ad hoc archives whenever data must move between endpoints.

Structured result fields:

- `source`
- `destination`
- `source_type`
- `bytes_copied`
- `files_copied`
- `directories_copied`
- `replaced`

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
- `remote-exec-daemon-xp` is narrower than the main daemon.
- On XP targets, `tty: true` is rejected.
- On XP targets, `view_image` is unavailable.
- On XP targets, file transfer support is limited to regular files and directory trees.
- Do not assume symlink-heavy or special-file transfers work on XP.

## Common Mistakes

- Guessing target names instead of calling `list_targets`.
- Forgetting that `local` means the broker host, not the current Codex workspace.
- Running a command on one target and expecting it to read or write files on another target.
- Using shell tricks for cross-endpoint copy instead of `transfer_files`.
- Treating `destination.path` as a directory container instead of the exact final path.
- Sending non-absolute paths to `transfer_files`.
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
- Need a valid target name or PTY capability first: `list_targets`
