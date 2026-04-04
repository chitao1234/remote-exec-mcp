# Windows XP Daemon Design

Status: approved design captured in writing

Date: 2026-04-04

References:

- `README.md`
- `docs/local-system-tools.md`
- `docs/superpowers/specs/2026-04-02-windows-support-design.md`
- `docs/superpowers/specs/2026-04-03-windows-shell-and-pty-design.md`
- `crates/remote-exec-broker/src/config.rs`
- `crates/remote-exec-broker/src/daemon_client.rs`
- `crates/remote-exec-broker/src/tools/transfer.rs`
- `crates/remote-exec-daemon/src/config.rs`
- `crates/remote-exec-daemon/src/exec/mod.rs`
- `crates/remote-exec-daemon/src/patch/mod.rs`
- `crates/remote-exec-daemon/src/server.rs`
- `crates/remote-exec-daemon/src/transfer/mod.rs`
- `crates/remote-exec-proto/src/public.rs`
- `crates/remote-exec-proto/src/rpc.rs`

## Goal

Add a standalone Windows XP daemon implementation that can serve the existing broker-facing `/v1/*` RPC contract, while intentionally supporting only a reduced v1 feature set.

This batch is a compatibility extension for an old Windows target, not a redesign of the broker or public MCP tool surface.

## Scope

Included:

- a new standalone XP daemon project in this repo
- same broker-facing HTTP endpoints and JSON payloads as the existing daemon
- plain HTTP transport for the XP daemon
- explicit broker config opt-in for insecure HTTP targets
- `exec_command` and `write_stdin` using Win32 process and pipe APIs
- `apply_patch` implemented directly in the XP daemon
- `transfer_files` support limited to single regular files
- truthful capability reporting for `supports_pty` and `supports_image_read`
- build wiring for `i686-w64-mingw32` on this host
- documentation for the new target type and insecure transport opt-in

Excluded:

- PTY support
- image read support
- directory transfer support on XP
- full error-message parity with the Rust daemon
- TLS transport in v1
- broad public MCP schema changes
- replacing the existing Rust daemon for modern Windows or Linux targets

## User Constraints

The design must honor these explicit constraints:

- use a standalone implementation rather than folding XP support into `remote-exec-daemon`
- keep the existing broker-facing protocol shape
- enforce the single-file transfer restriction in the XP daemon, not in the broker
- allow plain HTTP for XP v1, but require an explicit insecure opt-in in broker config
- use the host `i686-w64-mingw32` toolchain for builds
- prefer static linking where practical

The user also requested OpenSSL 3.0 as the TLS library family for XP compatibility. Because v1 transport is intentionally plain HTTP, OpenSSL is not a runtime requirement for the first cut. The project should still treat OpenSSL 3.0 as the chosen future TLS dependency for this daemon line rather than introducing a different TLS stack.

## Current State

Today the repo already has:

- a Rust daemon over mTLS JSON/HTTP
- a broker that assumes TLS credentials for every configured remote target
- a transfer pipeline that supports files and directories
- image-read support on the existing daemon
- platform-aware work for modern Windows targets inside the Rust codebase

Those pieces are useful reference points, but they are not suitable for an XP-first implementation because:

- the current daemon stack depends on modern Rust libraries and runtime assumptions
- the existing transfer implementation supports directories, which the XP v1 explicitly does not
- the existing transport model requires TLS client and server material, while the XP v1 uses plain HTTP

## Approaches Considered

### 1. Fold XP support into `remote-exec-daemon`

Pros:

- one daemon binary name
- shared Rust handlers and tests

Cons:

- does not fit the user's explicit standalone-project requirement
- pushes XP constraints into the modern daemon code path
- would still leave the underlying Rust/runtime compatibility problem unsolved

### 2. Standalone C++ XP daemon with the same `/v1/*` contract

Pros:

- fits the user's standalone-project requirement
- keeps broker and public tool changes narrow
- fastest path to a functional XP-targeted daemon
- isolates old-platform constraints from the modern Rust daemon

Cons:

- creates a second daemon implementation to maintain
- requires a separate build and test lane

### 3. Reduced XP protocol plus broker adaptation

Pros:

- slightly simpler daemon internals

Cons:

- violates the user's explicit protocol requirement
- increases broker complexity without real payoff
- creates target-specific RPC branching where the current architecture expects one contract

## Decision Summary

Use approach 2.

Add a standalone XP-focused C++ daemon project that preserves the existing `/v1/*` RPC contract, while explicitly narrowing behavior on XP:

- `supports_pty = false`
- `supports_image_read = false`
- transfer only regular files
- plain HTTP only in v1

The broker remains the only public MCP server and keeps ownership of public session IDs. The XP daemon owns only its local session IDs and internal process management.

## Architecture

### 1. New standalone project

Add a new repo-local project at:

- `crates/remote-exec-daemon-xp/`

This project should be built outside the Rust workspace's normal crate graph. It can still live under `crates/` for discoverability and repo locality, but it should have its own build files and toolchain assumptions.

Recommended contents:

- `crates/remote-exec-daemon-xp/src/`
- `crates/remote-exec-daemon-xp/include/`
- `crates/remote-exec-daemon-xp/Makefile` or similarly simple build entrypoint
- `crates/remote-exec-daemon-xp/README.md`
- `crates/remote-exec-daemon-xp/config/daemon-xp.example.ini` or `.toml`

The exact on-disk layout can stay simple. The important part is that the XP daemon remains operationally separate from the Rust daemon.

### 2. Preserve the existing daemon RPC surface

The XP daemon should expose the same broker-facing endpoints:

- `/v1/health`
- `/v1/target-info`
- `/v1/exec/start`
- `/v1/exec/write`
- `/v1/patch/apply`
- `/v1/transfer/export`
- `/v1/transfer/import`
- `/v1/image/read`

Endpoint meanings remain the same, but behavior may be reduced where the contract already supports capability differences.

### 3. Keep broker/session ownership unchanged

No change to the main architecture split:

- broker owns public opaque `session_id`
- XP daemon owns daemon-local session IDs
- broker validates target selection and routes requests
- `write_stdin` still resolves through the broker's session store

The XP daemon is a transport-compatible backend, not a second broker.

## Broker And Transport Design

### 1. Explicit insecure HTTP opt-in

Extend broker target config with an explicit flag:

- `allow_insecure_http = true`

Behavior:

- if `base_url` uses `http://`, this flag must be `true`
- if `base_url` uses `http://` without the flag, broker config loading fails early
- if `base_url` uses `https://`, current TLS expectations remain intact

This keeps insecure transport intentional and visible in operator config.

### 2. Per-target transport selection

Broker daemon-client construction should branch by target URL scheme:

- `https://...` targets use the current TLS client path
- `http://...` targets use a plain HTTP client path and ignore TLS material at request time

The broker should still keep the same higher-level request methods:

- `target_info`
- `exec_start`
- `exec_write`
- `patch_apply`
- `transfer_export_to_file`
- `transfer_import_from_file`
- `image_read`

That keeps the rest of the broker code mostly transport-neutral.

### 3. Config compatibility rule

The cleanest v1 config rule is:

- keep existing TLS fields for all current HTTPS targets
- add `allow_insecure_http` as an optional field defaulting to `false`
- allow HTTP targets to omit TLS file paths

This is a broker-config change, not a public MCP change.

## XP Daemon Behavior

### 1. Capability reporting

`/v1/target-info` should report:

- `platform = "windows"`
- `arch = "x86"` or equivalent stable 32-bit identifier used consistently by the daemon
- `supports_pty = false`
- `supports_image_read = false`

That makes the reduced feature set explicit without needing new schema fields.

### 2. Exec model

`exec_command` behavior on XP:

- `tty=true` is rejected
- default shell is `cmd.exe`
- the daemon uses `cmd.exe /C <cmd>` for v1 execution
- if `shell` is provided, only `cmd` and `cmd.exe` need to be accepted in v1
- `login` does not need full parity; the daemon may ignore it for `cmd.exe`

Implementation approach:

- `CreateProcessA` or `CreateProcessW` with redirected standard handles
- anonymous pipes for stdin/stdout/stderr
- non-blocking-ish output polling via `PeekNamedPipe`
- exit detection via `WaitForSingleObject`
- process cleanup via handle closure and termination on session eviction if needed

### 3. Write-stdin model

`write_stdin` should work for running sessions that still have an open stdin pipe.

This does not require PTY support. It only requires:

- session lookup by daemon-local session ID
- writing bytes into the child stdin pipe
- polling fresh stdout/stderr output afterward
- returning the standard `ExecResponse` shape

### 4. Session store

Keep the same high-level behavior contract:

- daemon-local opaque session IDs
- in-memory live-session map
- running/exited detection
- bounded output snapshots

The XP daemon does not need to copy every pruning or warning rule from the Rust daemon in v1, but it should keep enough session bookkeeping for the broker contract to work predictably.

### 5. `apply_patch`

The XP daemon implements `apply_patch` internally rather than shelling out.

Supported patch features for v1:

- add file
- delete file
- update file
- move file
- `*** End of File`

The parser does not need to chase full parity with the Rust implementation, but it must correctly handle the forms already used by this repo and by normal Codex-style patch generation.

Verification rules can stay intentionally lightweight:

- resolve workdir and target paths
- reject obvious path traversal outside the effective root
- reject unsupported patch structures
- read existing file contents before update/delete operations
- compute final updated file content before mutation where practical

Execution rules:

- create parent directories for adds and moves when needed
- write updated content to a temp file and rename into place where practical
- remove deleted sources after successful move/write
- return a simple success summary string

### 6. `transfer_files`

The XP daemon keeps the same export/import endpoints, but only for regular files.

Export rules:

- absolute source path required
- source must exist
- source must be a regular file
- response header `x-remote-exec-source-type` is `file`
- body is the raw file bytes

Import rules:

- request `source_type` must be `file`
- destination is the exact final file path
- overwrite behavior follows the existing fail/replace mode concept
- response reports one file copied and zero directories copied

Rejected cases:

- any directory export
- any directory import
- archive-style multi-entry transfer

This restriction is enforced by the XP daemon only, per the user's direction. The broker remains general.

### 7. `view_image`

`/v1/image/read` should stay present for protocol compatibility but return a normal RPC error indicating the feature is unsupported on this target class.

This is paired with `supports_image_read = false`.

## Build And Packaging

### 1. Toolchain

Build the XP daemon on this host with the `i686-w64-mingw32` toolchain.

The implementation should favor straightforward native dependencies that can be produced by that toolchain without requiring a modern MSVC environment.

### 2. Language choice

Use C++ for the standalone XP daemon.

Reason:

- faster prototyping than C for string handling, JSON/HTTP plumbing, and session bookkeeping
- still close enough to Win32 APIs for a small native daemon
- easier to keep the code compact without pulling in heavy abstractions

### 3. Linking preference

Prefer static linking where practical, while accepting that some platform libraries may remain dynamic if the MinGW/XP toolchain requires it.

The goal is operational simplicity, not a perfect fully-static artifact at any cost.

### 4. OpenSSL 3.0 position

For v1, the daemon runs plain HTTP and does not require TLS termination at runtime.

Even so, the XP daemon line should standardize on OpenSSL 3.0 for future HTTPS work rather than introducing a second TLS library family. That means any build notes or dependency scaffolding should treat OpenSSL 3.0 as the intended TLS dependency for later phases.

## Testing

The XP daemon will need a separate verification lane from the Rust workspace tests.

At minimum, test coverage should prove:

- `/v1/health` and `/v1/target-info` respond with the expected JSON shape
- broker config rejects `http://` targets unless `allow_insecure_http = true`
- broker can call an insecure XP target when the flag is enabled
- `exec_command` can start a non-PTY `cmd.exe` process and return output
- `write_stdin` can write to a running redirected-stdin session
- `tty=true` is rejected by the XP daemon
- `apply_patch` can add, update, move, and delete files
- `transfer/export` and `transfer/import` succeed for a single file
- directory transfer attempts fail at the XP daemon
- `/v1/image/read` reports unsupported

The repo does not need to force these checks through `cargo test --workspace`. The XP daemon should define its own focused build and verification commands.

## Docs Impact

Update project docs together with the code:

- `README.md`
  - mention the XP daemon as a separate target class
  - document the insecure HTTP opt-in requirement
  - document the XP v1 limitations
- broker config examples
  - show how an XP target is configured with `http://...` plus `allow_insecure_http = true`
- XP daemon local docs
  - document build prerequisites, config format, and runtime limitations

## Risks And Deliberate Tradeoffs

- Two daemon implementations will exist in one repo.
- XP v1 intentionally gives up TLS, PTY, image read, and directory transfers.
- Error strings will not fully match the Rust daemon.
- The build and test story will be split between Cargo-driven Rust code and a separate MinGW-driven XP daemon lane.

Those tradeoffs are acceptable because the user's goal is a fast, narrow, operable XP target rather than a full parity platform port.
