# Windows Shell And PTY Follow-Up Design

Status: approved design captured in writing

Date: 2026-04-03

References:

- `README.md`
- `docs/superpowers/specs/2026-04-02-windows-support-design.md`
- `crates/remote-exec-daemon/Cargo.toml`
- `crates/remote-exec-daemon/src/exec/mod.rs`
- `crates/remote-exec-daemon/src/exec/session.rs`
- `crates/remote-exec-daemon/src/exec/shell.rs`
- `crates/remote-exec-daemon/tests/exec_rpc.rs`

## Goal

Refine the Windows exec implementation before ConPTY testing by tightening default shell selection, defining Windows login behavior, and adding a `winpty-rs` fallback for `tty=true`.

This batch is an internal compatibility update. The public MCP tool surface stays unchanged.

## Scope

Included:

- Windows default shell selection when `exec_command.shell` is omitted
- Windows login semantics for `login=true`, `login=false`, and omitted `login`
- Windows PTY backend fallback from `portable-pty` to `winpty-rs`
- truthful `supports_pty` reporting on Windows after both backends are considered
- tests and docs needed to lock in the new behavior

Excluded:

- broker public API changes
- shell parsing changes beyond the existing shell-family detection
- non-Windows shell behavior changes
- ConPTY feature expansion beyond trying the existing backend first
- edits to `docs/local-system-tools.md`

## Current Behavior Summary

The current Windows support gets the code compiling and passing the Wine-target checks, but three Windows-specific behaviors are still too weak:

- default shell selection falls back directly to `COMSPEC` or `cmd.exe`
- Windows treats `login=true` as unsupported instead of defining behavior
- `tty=true` only depends on the `portable-pty` backend and does not try `winpty-rs` when ConPTY is unavailable

Those gaps are acceptable for initial compatibility work but not for the next round of Windows validation.

## Approaches Considered

### 1. Extend the current exec modules in place

Update shell resolution and PTY spawning inside the existing daemon exec modules, while keeping any new Windows-only logic behind small internal helpers.

Pros:

- smallest diff
- keeps existing request flow and session ownership unchanged
- fastest path to a real Windows runner

Cons:

- some platform branching remains in the current exec area

### 2. Split Windows behavior into new dedicated modules

Move Windows shell selection and PTY spawning into separate files now.

Pros:

- cleaner long-term separation

Cons:

- more churn than the current scope needs
- higher risk while the Windows implementation is still settling

## Decision Summary

Use approach 1.

Keep the handler and session model intact. Add the smallest internal seams needed to make Windows shell selection and PTY fallback explicit and testable.

## Shell Selection Design

### 1. Explicit shell override stays highest priority

If the request provides `shell`, the daemon uses it directly on Windows just as it does today.

### 2. Windows default shell order becomes PowerShell-first

When `shell` is omitted on Windows, resolve in this order:

1. first `pwsh.exe` found on `PATH`
2. first `powershell.exe` or `powershell` found on `PATH`
3. `COMSPEC` if it is present and non-empty
4. `cmd.exe`

The implementation should search `PATH` lexically and use the first matching executable name found there. It should not probe versioned installation directories outside `PATH`.

### 3. Unix shell resolution stays unchanged

The Unix path through `SHELL`, passwd shell, `bash`, and `/bin/sh` remains as it is today.

## Windows Login Design

### 1. Windows no longer rejects `login=true`

Windows should stop reporting `login_shell_unsupported`. Instead, the daemon should interpret `login` as whether to suppress profile and AutoRun behavior.

### 2. `login` keeps the existing daemon policy contract

The decision remains:

- `login: Some(true)` => request login behavior unless disabled by config
- `login: Some(false)` => request non-login behavior
- `login: None` => follow `allow_login_shell`

If `allow_login_shell = false`, `login: Some(true)` should still fail with the existing config-driven error.

### 3. PowerShell-family behavior

For `pwsh`, `pwsh.exe`, `powershell`, and `powershell.exe`:

- `login=false` => include `-NoProfile`
- `login=true` => omit `-NoProfile`

The command still runs through `-Command`.

### 4. `cmd.exe` behavior

For `cmd.exe` and `cmd`:

- `login=false` => use `/D /C` so AutoRun commands are suppressed
- `login=true` => use `/C`

This gives Windows a defined profile/AutoRun suppression model without inventing Unix-style login shell semantics.

## PTY Backend Design

### 1. Backend order on Windows

For `tty=true`, Windows should attempt PTY backends in this order:

1. `portable-pty` using the existing path
2. `winpty-rs`
3. if both are unavailable, return `tty is not supported on this host`

### 2. Session contract stays unchanged

The common `LiveSession` contract still needs to support:

- spawn
- read available output
- detect exit
- terminate
- write to stdin for TTY sessions

The handler in `exec/mod.rs` should not gain backend-specific branching beyond calling the session layer.

### 3. Capability reporting becomes backend-aware

`supports_pty()` on Windows should report `true` if either backend can be initialized successfully. This keeps `TargetInfoResponse.supports_pty` aligned with actual runtime behavior.

## Testing

Add or update Windows-target coverage for:

- omitted-shell resolution preferring `pwsh.exe`
- fallback from `pwsh.exe` to older PowerShell and then to `cmd.exe`
- PowerShell argv with and without `-NoProfile`
- `cmd.exe` argv with `/D /C` for `login=false`
- Windows `login=true` succeeding when config allows it
- Windows `login=true` still failing when daemon config disables login shells
- PTY capability checks that prefer `portable-pty` and fall back to `winpty-rs`

Wine remains useful for compile- and argv-level verification, but the PTY backend fallback must still be validated on a real Windows runner.

## Docs Impact

Update project-owned docs that describe Windows support and exec behavior, especially `README.md`, if the current wording still says Windows login shells are unsupported or implies ConPTY is the only PTY path.
