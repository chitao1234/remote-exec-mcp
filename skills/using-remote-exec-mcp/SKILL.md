---
name: using-remote-exec-mcp
description: Use when work must happen on a named remote target or optional broker-host `local` target, when files must move between endpoints with `transfer_files`, or when Codex-style command, patch, stdin, or image workflows must run through remote-exec-mcp instead of local tools
---

# Using remote-exec-mcp

## Overview

`remote-exec-mcp` is a specialized toolset for target-scoped work. Use it when the work belongs on a named target, when the broker exposes its own host as `target: "local"`, or when bytes must move between endpoints. Do not treat it as the default toolset for ordinary local-only tasks.

For Codex agents, `exec_command`, `write_stdin`, `apply_patch`, and `view_image` map closely to the internal local-system tools. The extra concerns are choosing the correct `target`, checking target capabilities when needed, and using `transfer_files` whenever bytes must cross endpoint boundaries.

## When to Use

- A command needs to run on a named remote target.
- The broker-host `local` target is enabled and the work belongs on that host rather than in your current workspace.
- A file on a target needs to be inspected or changed.
- A live remote session needs more input after the first command call.
- Files or directories need to move between `local` and a remote target.
- Files or directories need to move between two remote targets.
- An image that already exists on a target needs inspection.

Do not use this skill for purely local work.

## Tool Selection Guide

- `list_targets`: discover valid target names and any cached platform or PTY metadata before making remote calls.
- `exec_command`: run a command on one target.
- `write_stdin`: continue a live session returned by `exec_command`.
- `apply_patch`: edit files directly on one target.
- `view_image`: inspect an image file on one target.
- `transfer_files`: move files or directories between `local` and remote targets, between two remote targets, or between two paths on `local`.

## Target Model

- Treat target selection as full access on that machine.
- `local` is special: it means the broker host, not your current local Codex workspace. Use it only when you intentionally want broker-host process or filesystem access.
- For `exec_command`, `write_stdin`, `apply_patch`, and `view_image`, `local` exists only when the broker-host local target is enabled. For `transfer_files`, `local` is the broker-host endpoint and can be used even when it is not listed by `list_targets`.
- Call `list_targets` instead of guessing. If the broker-host exec target is enabled, it appears there as `local`.
- `list_targets` is inventory, not a live health probe. Missing `daemon_info` means the broker has no current cached metadata for that target; it does not by itself mean the target name is invalid.

## Practical Patterns

### `list_targets`

- Call it early instead of guessing target names.
- Reuse the exact returned target name in later tool calls.
- Use cached `platform`, `arch`, and `supports_pty` data to decide whether `tty=true` makes sense before starting an interactive session.

### `exec_command`

- Be explicit about `target` and intentional about `workdir`.
- Prefer straightforward non-interactive commands for inspection, builds, tests, and file discovery.
- Do not send patch text through `exec_command`. Use `apply_patch` directly; the broker only intercepts that pattern to preserve compatibility and still warns.
- Use a longer-lived session only when the command actually needs interaction or follow-up polling.
- PTY is target-dependent. Check `list_targets` when you need `tty=true`.
- Keep the returned `session_id` when the command stays alive and more input or output will follow.

### `write_stdin`

- Use it only with a valid active `session_id` from `exec_command`.
- Use it for prompts, shells, REPLs, editors, and other long-running interactive programs.
- Send empty input when you only need to poll for more output from an active session.
- If the tool reports `Unknown process id ...`, treat that session as gone and start a new one.

### `apply_patch`

- Prefer it over ad hoc shell editing for targeted file changes on one remote target.
- Use the same editing discipline as the internal Codex tool: explicit diffs, focused edits, and no shell redirection as a substitute for patching.
- The current patch engine supports the documented `*** End of File` marker.
- Pair it with `exec_command` when you need inspection before the edit or verification after the edit.

### `view_image`

- Use it when the image already exists on the target you are working on.
- Omit `detail` for the default resized or passthrough behavior. Use `detail: "original"` only when exact original bytes or full-fidelity inspection matters.
- `view_image.detail` currently supports only `original`; other values are rejected.
- Some targets may not implement image reads. In particular, `remote-exec-daemon-xp` does not support `view_image`.
- Do not transfer an image just to inspect it unless another workflow requires the image to move.

### `transfer_files`

- Use it whenever bytes must cross endpoint boundaries.
- Common cases: upload a local script, config, or fixture to a remote target; download logs or generated artifacts; copy content from one remote target to another; copy content to or from broker-host `local`.
- Reach for `transfer_files` instead of trying to fake a copy with shell commands that only execute on one endpoint.
- Source and destination paths must be absolute for their own endpoint platform.
- Treat `destination.path` as the exact final path you want to create or replace. It does not infer a basename and it does not copy "into" an existing directory for you.
- `overwrite` controls whether an existing destination is rejected or replaced.
- `create_parent` only creates missing parent directories for the exact destination path you chose.
- The source and destination must be distinct endpoints.
- Windows endpoints accept normal drive-qualified paths and also MSYS/Cygwin-style absolute paths such as `/c/work/file.txt` and `/cygdrive/c/work/file.txt`.
- `remote-exec-daemon-xp` supports only regular files and directory trees for transfer.

## Common Remote Workflows

### Inspect And Edit Remote Code

1. Call `list_targets`.
2. Use `exec_command` to inspect files, search the tree, or run status commands on the target.
3. Use `apply_patch` to edit the remote file directly.
4. Use `exec_command` again for tests, formatting, or verification on that same target.

### Upload, Run, Retrieve

1. Use `transfer_files` to copy local input to the remote target, choosing the exact destination path.
2. Use `exec_command` on that target to run the program or script.
3. Use `transfer_files` again if results need to come back to `local`.

### Continue An Interactive Remote Program

1. Start it with `exec_command`.
2. Make sure the target supports PTY first if the program expects a TTY.
3. Keep the returned `session_id`.
4. Use `write_stdin` to answer prompts or continue the session until it exits.

### Inspect A Remote Image

1. Use `exec_command` first if you need to locate the image path.
2. Use `view_image` on that target and path.

### Move Content Between Remote Targets

1. Use `transfer_files` with a remote source target and a different remote destination target.
2. Use `exec_command` on the destination target if you need to verify or use the moved content.

## Common Mistakes

- Guessing target names or target capabilities instead of calling `list_targets`.
- Using `write_stdin` without a live session.
- Editing through shell commands when `apply_patch` is the better fit.
- Forgetting that one command runs on one target only; cross-endpoint movement should use `transfer_files`.
- Assuming `local` means your current Codex workspace instead of the broker host.
- Assuming `destination.path` means "copy into this directory" instead of "create or replace exactly this final path".
- Sending non-absolute endpoint paths to `transfer_files`.
- Passing unsupported `view_image.detail` values instead of omitting `detail` or using `original`.
- Treating `remote-exec-mcp` as the default local toolset instead of a specialized remote one.
