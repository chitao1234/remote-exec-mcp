---
name: using-remote-exec-mcp
description: Use when work must happen on a named remote target, files need to move between local and remote endpoints, or Codex-style command, patch, stdin, or image workflows must run on a remote machine
---

# Using remote-exec-mcp

## Overview

`remote-exec-mcp` is a specialized toolset for remote work. Use it when the work itself belongs on a named target or when files must move between endpoints. Do not treat it as the default toolset for ordinary local-only tasks.

For Codex agents, use `exec_command`, `write_stdin`, `apply_patch`, and `view_image` the same way you use the internal local-system tools. The extra concerns here are choosing the correct `target` and using `transfer_files` when bytes must cross endpoints.

## When to Use

- A command needs to run on a named remote target.
- A file on a remote target needs to be inspected or changed.
- A live remote session needs more input after the first command call.
- Files or directories need to move between `local` and a remote target.
- Files or directories need to move between two remote targets.
- An image that already exists on a remote target needs inspection.

Do not use this skill for purely local work.

## Tool Selection Guide

- `list_targets`: discover valid target names before making remote calls.
- `exec_command`: run a command on one target.
- `write_stdin`: continue a live session returned by `exec_command`.
- `apply_patch`: edit files directly on one target.
- `view_image`: inspect an image file on one target.
- `transfer_files`: move files or directories between `local` and remote targets, or between two remote targets.

## Practical Patterns

### `list_targets`

- Call it early instead of guessing target names.
- Reuse the exact returned target name in later tool calls.

### `exec_command`

- Be explicit about `target` and intentional about `workdir`.
- Prefer straightforward non-interactive commands for inspection, builds, tests, and file discovery.
- Use a longer-lived session only when the command actually needs interaction or follow-up polling.
- Keep the returned `session_id` when the command stays alive and more input or output will follow.

### `write_stdin`

- Use it only with a valid active `session_id` from `exec_command`.
- Use it for prompts, shells, REPLs, editors, and other long-running interactive programs.
- Send empty input when you only need to poll for more output from an active session.

### `apply_patch`

- Prefer it over ad hoc shell editing for targeted file changes on one remote target.
- Use the same editing discipline as the internal Codex tool: explicit diffs, focused edits, and no shell redirection as a substitute for patching.
- Pair it with `exec_command` when you need inspection before the edit or verification after the edit.

### `view_image`

- Use it when the image already exists on the target you are working on.
- Do not transfer an image just to inspect it unless another workflow requires the image to move.

### `transfer_files`

- Use it whenever bytes must cross endpoint boundaries.
- Common cases: upload a local script, config, or fixture to a remote target; download logs or generated artifacts; copy content from one remote target to another.
- Reach for `transfer_files` instead of trying to fake a copy with shell commands that only execute on one endpoint.
- Treat `destination.path` as the exact final path you want to create or replace.

## Common Remote Workflows

### Inspect And Edit Remote Code

1. Call `list_targets`.
2. Use `exec_command` to inspect files, search the tree, or run status commands on the target.
3. Use `apply_patch` to edit the remote file directly.
4. Use `exec_command` again for tests, formatting, or verification on that same target.

### Upload, Run, Retrieve

1. Use `transfer_files` to copy local input to the remote target.
2. Use `exec_command` on that target to run the program or script.
3. Use `transfer_files` again if results need to come back to `local`.

### Continue An Interactive Remote Program

1. Start it with `exec_command`.
2. Keep the returned `session_id`.
3. Use `write_stdin` to answer prompts or continue the session until it exits.

### Inspect A Remote Image

1. Use `exec_command` first if you need to locate the image path.
2. Use `view_image` on that target and path.

### Move Content Between Remote Targets

1. Use `transfer_files` with a remote source target and a different remote destination target.
2. Use `exec_command` on the destination target if you need to verify or use the moved content.

## Common Mistakes

- Guessing target names instead of calling `list_targets`.
- Using `write_stdin` without a live session.
- Editing through shell commands when `apply_patch` is the better fit.
- Forgetting that one command runs on one target only; cross-endpoint movement should use `transfer_files`.
- Treating `remote-exec-mcp` as the default local toolset instead of a specialized remote one.
