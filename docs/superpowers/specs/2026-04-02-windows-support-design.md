# Windows Support Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `README.md`
- `docs/local-system-tools.md`
- `docs/specs/2026-03-31-remote-exec-mcp-design.md`
- `docs/superpowers/specs/2026-04-02-transfer-files-design.md`
- `crates/remote-exec-broker/src/lib.rs`
- `crates/remote-exec-broker/src/local_transfer.rs`
- `crates/remote-exec-broker/src/mcp_server.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- `crates/remote-exec-broker/src/tools/transfer.rs`
- `crates/remote-exec-daemon/src/config.rs`
- `crates/remote-exec-daemon/src/exec/locale.rs`
- `crates/remote-exec-daemon/src/exec/mod.rs`
- `crates/remote-exec-daemon/src/exec/session.rs`
- `crates/remote-exec-daemon/src/exec/shell.rs`
- `crates/remote-exec-daemon/src/server.rs`
- `crates/remote-exec-daemon/src/transfer/archive.rs`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`

## Goal

Add Windows support for the existing remote-exec architecture without widening the public MCP tool surface.

This batch must support:

- a broker running on Windows
- Windows daemons as configured targets
- `exec_command`
- `write_stdin`
- `transfer_files`

The goal is compatibility expansion, not a redesign of the public API. Linux behavior remains supported, while Windows becomes a first-class platform for both broker-local operations and remote target execution.

## Scope

Included:

- Windows broker-host support for `target: "local"` operations
- Windows daemon support for `exec_command`, `write_stdin`, and `transfer_files`
- `tty=true` support on Windows through a PTY-capable backend
- endpoint-aware path parsing and path identity comparison
- Windows separator normalization for transfer paths
- best-effort executable preservation only on OSes that support it
- dynamic PTY capability reporting through existing target metadata
- docs and tests needed to make Windows behavior explicit

Excluded:

- macOS target support
- public schema changes for existing tools
- approval or sandbox model changes
- ACL, owner, xattr, or timestamp fidelity work
- emulating Unix executable bits on Windows
- broad shell-parsing improvements beyond documented compatibility forms
- sync or staging extensions to `transfer_files`

## Current Behavior Summary

The repository currently describes itself as Linux-only, and the code reflects that in the highest-risk places:

- broker-local transfer logic imports `std::os::unix::fs::PermissionsExt`
- daemon transfer logic imports `std::os::unix::fs::PermissionsExt`
- transfer tests and end-to-end tests assert Unix executable bits directly
- daemon shell resolution prefers `SHELL`, passwd shell lookup, `bash`, and `/bin/sh`
- daemon locale probing shells out to `locale -a`
- daemon PTY handling uses `portable-pty`, but target metadata hardcodes `supports_pty: true`
- broker transfer path validation uses host `Path::is_absolute()` and host lexical normalization for all endpoints
- broker `exec_command` interception is still oriented around Unix-style wrappers even though the compatibility notes already mention Windows shell forms

Those assumptions are correct for the current Linux-only product, but they are wrong for a Windows broker host and wrong for Windows remote targets.

## Approaches Considered

### 1. In-place `cfg` branching inside current files

Add `#[cfg(windows)]` branches directly inside the current exec and transfer modules.

Pros:

- smallest initial diff
- fastest way to get basic compilation

Cons:

- platform logic spreads across broker transfer, daemon exec, shell resolution, and tests
- path rules and executable-bit rules become harder to reason about
- future extensions, especially eventual macOS support, would compound the mess

### 2. Platform adapters behind stable internal contracts

Keep the public tools unchanged and split platform-sensitive behavior behind internal exec and filesystem helper layers.

Pros:

- isolates Windows complexity
- preserves current broker and session architecture
- keeps transfer path rules and executable preservation consistent between broker-local and daemon code
- leaves room for future macOS work without encoding bad assumptions now

Cons:

- larger internal refactor than scattered `cfg` fixes

### 3. Public capability redesign

Add richer public capability metadata for path style, login-shell support, executable preservation, and shell families.

Pros:

- most explicit public contract
- easiest to extend later

Cons:

- larger scope
- more public schema churn
- unnecessary for the immediate Windows target

## Decision Summary

Use approach 2.

Keep the public tool surface stable and introduce platform-aware internal adapters for:

- exec/session behavior
- path parsing and same-path comparison
- local filesystem transfer behavior
- capability reporting

Do not expand the public schema in this batch.

## Public API

The public MCP surface does not change:

- `exec_command`
- `write_stdin`
- `transfer_files`

Their argument and result schemas stay as they are today.

The existing `platform`, `arch`, and `supports_pty` daemon metadata remain the compatibility surface for platform-aware broker behavior.

## Platform Strategy

### 1. Keep the current broker/daemon split

The architectural split remains intact:

- broker owns public opaque `session_id` values
- daemons own local exec sessions
- broker validates target selection and routes the request
- `transfer_files` remains broker-mediated for every direction

Windows support must fit inside that model rather than bypass it.

### 2. Add internal platform backends, not public platform switches

The implementation should split platform-sensitive behavior into backend/helper layers rather than scattering conditional code through request handlers.

At minimum there should be internal platform seams for:

- shell resolution and shell argv construction
- PTY spawn/write/poll behavior
- deterministic environment normalization
- absolute-path validation
- path normalization and same-path comparison
- executable preservation policy

The handler layer should remain mostly platform-neutral.

### 3. Preserve Linux behavior while extending Windows

The Windows work is an additive compatibility batch, not a Linux behavior rewrite.

Linux remains the reference behavior for:

- Unix login shells
- locale probing and locale env shaping
- executable bit preservation
- symlink rejection semantics already implemented for transfer

## Exec Design

### 1. Broker behavior stays stable

For both Linux and Windows targets:

- `exec_command` still creates broker-owned opaque `session_id` values when the daemon returns a running session
- `write_stdin` still routes only through the owning daemon session record
- text output and structured result shapes stay unchanged
- timeout clamps and polling rules stay unchanged

The broker should not gain OS-specific session logic.

### 2. Daemon exec becomes platform-backed

The daemon exec implementation should expose a stable session contract to the handlers, with platform-specific backends underneath.

The backend contract must cover:

- spawn with `tty=true`
- spawn with `tty=false`
- non-blocking output reads
- exit detection
- termination
- stdin writes for TTY sessions

The handler in `exec/mod.rs` should continue to own the common response assembly, output truncation, and session-store integration.

### 3. Use `portable-pty` on Windows too

`portable-pty` already exposes a Windows backend through ConPTY support.

The Windows PTY implementation should use that backend rather than inventing a second PTY/session protocol. This keeps Linux and Windows TTY handling on one library abstraction.

### 4. Make PTY capability dynamic and truthful

`TargetInfoResponse.supports_pty` must stop being hardcoded.

Instead:

- Linux daemons report `true`
- Windows daemons report whether ConPTY-backed PTY support is actually available on that host/runtime

If a Windows host cannot provide PTY support:

- `supports_pty` becomes `false`
- `tty=true` requests fail clearly
- `tty=false` continues to work

The actual exec call remains authoritative even when metadata says PTY is supported.

### 5. Windows shell resolution is backend-specific

Windows shell handling cannot reuse the Unix `-c` / `-l -c` assumptions.

Windows behavior:

- if `shell` is provided, use it with shell-family-aware argv construction when recognized
- if `shell` is omitted, prefer `COMSPEC`
- if `COMSPEC` is absent or unusable, fall back to `cmd.exe`
- explicit PowerShell or `pwsh` remains allowed through the existing `shell` field

Windows shell argv derivation should recognize the common Windows shell families documented in the compatibility notes:

- `cmd.exe` via `/C`
- `powershell.exe` or `pwsh` via `-Command`

This batch does not need general support for arbitrary Windows shell executables with unknown argument conventions.

### 6. Windows login-shell semantics are unsupported in v1

Windows does not get Unix-style login-shell semantics in this batch.

Rules:

- explicit `login=true` on a Windows target returns a clear error
- omitted `login` on Windows resolves to non-login behavior rather than inheriting the Unix defaulting rules
- `allow_login_shell` remains meaningful on Unix targets and is effectively Unix-only in v1

This avoids the broken outcome where Windows daemons would reject ordinary calls simply because the Unix default is `allow_login_shell = true`.

### 7. Deterministic environment shaping stays cross-platform where possible

The deterministic non-locale env overlay remains cross-platform:

- `NO_COLOR`
- `TERM`
- `COLORTERM`
- `PAGER`
- `GIT_PAGER`
- `GH_PAGER`
- `CODEX_CI`

Locale handling becomes platform-specific:

- Unix keeps the current `locale -a` discovery and `LANG` / `LC_*` shaping
- Windows skips Unix locale probing and does not promise `LANG` / `LC_*` normalization in v1

### 8. `write_stdin` contract stays the same

On Windows as on Linux:

- `write_stdin(chars="")` acts as a poll
- non-empty `chars` requires a TTY-backed session
- non-empty writes to non-TTY sessions return the same `stdin is closed... tty=true` error shape

The goal is to preserve the public behavioral contract while changing only the session backend.

### 9. Keep `apply_patch` interception compatible with Windows shell wrappers

Windows support for `exec_command` should also close the related compatibility gap in the broker-side interception logic.

The interception matcher should recognize the narrow documented wrapper families in addition to the existing Unix forms:

- `cmd /c`
- `powershell` / `pwsh -Command`

This remains a narrow compatibility parser, not a general-purpose shell parser. The accepted shell-body forms stay intentionally conservative.

## Transfer Design

### 1. Keep `transfer_files` as the only public transfer tool

Do not add new public transfer tools in this batch.

`transfer_files` remains the only public copy/move facade, with the existing endpoint shape:

```json
{
  "source": { "target": "...", "path": "..." },
  "destination": { "target": "...", "path": "..." },
  "overwrite": "fail" | "replace",
  "create_parent": true | false
}
```

### 2. Stop validating remote paths with the broker host OS rules

The current broker transfer code uses host `Path` behavior for all endpoints. That fails when:

- the broker is Windows and the remote endpoint is Linux
- the broker is Linux and the remote endpoint is Windows

Validation must become endpoint-aware.

Rules:

- `target: "local"` is validated with the broker-host path rules
- remote endpoints are validated with the target-platform path rules

The broker can only perform early checks when it knows the endpoint path semantics.

### 3. Accept both slash styles for Windows input

Windows path input may use either separator style:

- `C:\work\artifact.zip`
- `C:/work/artifact.zip`

Before any filesystem operation, Windows paths must be normalized into one internal representation on the handling side.

Important constraint:

- normalization must be path-aware
- do not implement this as naive global string replacement that could corrupt valid Windows prefixes or edge cases

### 4. Preserve the public exact-path contract

The public contract remains:

- `destination.path` is the exact final path to create or replace

Internal normalization is allowed before:

- equality checks
- overwrite checks
- destination preparation
- filesystem access

The structured result may continue to echo the caller-provided endpoint values; normalization is an internal implementation detail.

### 5. Separate path syntax normalization from path identity comparison

These are different problems and must not be conflated.

Path syntax normalization:

- Windows normalizes separator forms before use
- Unix keeps `/`

Path identity comparison:

- Windows comparisons are case-insensitive
- Linux comparisons are case-sensitive

Important future-proofing rule:

- do not encode this as `windows vs unix` forever
- use an internal path-comparison policy abstraction so the implementation does not bake in the false assumption that every Unix-family target is case-sensitive

This matters because common macOS filesystems are often case-insensitive even though macOS support is not in scope for this batch.

### 6. Same-path detection becomes platform-aware

The broker should still reject obvious self-transfer collisions, but only when it can compare paths correctly for the endpoint.

For each endpoint pair:

- local broker endpoints use broker-host path normalization and comparison rules
- known Windows endpoints normalize separators and compare case-insensitively
- known Linux endpoints use the current lexical comparison rules

If the broker cannot prove sameness safely, endpoint-side logic remains the final authority for preventing exact-path mutation hazards.

### 7. Share path and filesystem rules between broker-local and daemon transfer

Broker-local transfer and daemon transfer must agree on:

- what counts as an absolute path
- separator normalization
- same-path comparison
- overwrite behavior
- destination parent creation
- symlink rejection
- executable preservation policy

That should come from a shared internal rules layer rather than duplicated platform branches.

### 8. Keep the archive format stable

The relay format stays the same:

- tar archive
- one regular file or one directory tree
- no new public or private metadata beyond the current tar header fields

This batch is about OS compatibility, not archive redesign.

### 9. Executable preservation is best effort only on supporting OSes

Preservation rules become explicit:

- Unix export keeps mode bits in the tar header
- Unix import restores executable bits when present
- Windows import ignores Unix executable bits
- Windows export does not promise equivalent metadata fidelity

The public contract becomes:

- best effort preserve executable intent only when the destination OS supports it

### 10. Keep the strict symlink policy

V1 symlink behavior remains unchanged across platforms:

- reject symlink source roots
- reject nested symlinks inside directory trees
- do not copy, dereference, or recreate symlinks

Windows support does not expand symlink semantics.

## Absolute Path Semantics

Absolute path validation must follow the handling platform, not the broker host blindly.

Expected accepted forms include the platform-native absolute forms that the backend can safely handle, including:

- Linux-style absolute paths on Linux targets
- drive-letter absolute paths on Windows targets
- UNC or other Windows absolute forms when the backend path parser and OS APIs accept them

The implementation should not hardcode a narrow string-prefix checklist when the platform path parser already knows how to answer the question.

## Target Metadata

The existing `TargetInfoResponse` remains the compatibility channel for broker-side platform awareness.

The broker may use target metadata to select path and PTY behavior, including:

- `platform`
- `arch`
- `supports_pty`

No new public metadata is required in this batch.

## Testing Strategy

### 1. Keep tests aligned with platform responsibility

The test split should mirror the architecture:

- broker tests for routing, normalization decisions, and public-surface behavior
- daemon tests for local exec/session behavior
- daemon tests for local transfer behavior
- end-to-end tests for cross-component correctness

### 2. Add Windows-specific exec coverage

Daemon exec coverage should include Windows-specific tests for:

- omitted-shell fallback through `COMSPEC` or `cmd.exe`
- recognized Windows shell argv construction
- explicit `login=true` rejection
- `tty=false` pipe execution
- `tty=true` PTY-backed execution when supported
- `write_stdin` polling and stdin writes against a Windows PTY session

### 3. Add Windows-specific transfer coverage

Transfer coverage should include:

- Windows absolute path acceptance
- mixed slash and backslash input normalization
- same-path detection under Windows case-insensitive comparison
- overwrite behavior on Windows paths
- best-effort executable preservation no-op behavior on Windows

### 4. Make Unix-only assertions explicitly Unix-only

Current Unix-only tests must be gated explicitly instead of pretending to be portable.

Examples:

- `PermissionsExt` mode assertions
- direct `/bin/sh` expectations
- symlink creation helpers that depend on Unix behavior

The cross-platform suite should assert behavior that is genuinely cross-platform, while platform-specific suites carry the platform-specific checks.

### 5. Keep broker-local transfer tests portable

`local -> local` broker tests should be written so they can pass on both Linux and Windows broker hosts.

That means:

- no hardcoded Unix path assumptions
- no unconditional Unix executable-bit assertions
- separator and case-comparison behavior covered where relevant

### 6. Preserve Linux coverage

Windows support must not reduce the current Linux confidence level.

The Linux-focused exec and transfer tests remain part of the regression suite, with Unix-specific checks gated rather than removed.

### 7. Expect at least one Windows validation path

Because the current development environment is Linux-oriented, the final implementation must still be verified on a Windows host or Windows CI runner for the Windows-specific paths.

The design should therefore keep Windows-specific tests isolated enough to run under `cfg(windows)` without destabilizing Linux development loops.

## Documentation Changes

Update the docs in the same batch:

- `README.md`
  - remove the Linux-only headline
  - document Windows broker-host support
  - document Windows exec/login limitations
  - document Windows path separator normalization and best-effort executable preservation
- `docs/local-system-tools.md`
  - add notes where remote behavior intentionally diverges by platform
  - document Windows shell-family handling and unsupported login semantics

## Non-Goals

This batch intentionally does not include:

- macOS support
- new public capability fields
- transfer sync or staging features
- shell-agnostic behavior across every possible Windows shell
- executable-bit emulation on Windows
- symlink support expansion
- filesystem metadata fidelity beyond the current best-effort behavior

## Acceptance Criteria

This design is satisfied when:

- the broker can run on Windows and serve `target: "local"` for the supported tools
- a Windows daemon can serve `exec_command`, `write_stdin`, and `transfer_files`
- `tty=true` works on Windows when PTY capability exists
- `supports_pty` accurately reports the daemon capability instead of being hardcoded
- Windows transfer paths accept either slash style and normalize safely before filesystem access
- same-path detection respects Windows case-insensitive comparison
- Linux transfer behavior still preserves executable bits
- Windows transfer behavior performs no unsupported executable restoration
- public MCP schemas remain unchanged
- docs no longer claim Linux-only support

