# remote-exec-daemon-cpp

`remote-exec-daemon-cpp` is the standalone C++11 daemon for
`remote-exec-mcp`. It is useful when the Rust daemon is too large for the host
environment or when legacy Windows coverage is required. It supports explicit
plain HTTP and optional OpenSSL mutual TLS for direct listeners and reverse
connections. TLS requires OpenSSL 1.0.2 or newer, negotiates TLS 1.2 or newer,
and supports the OpenSSL 1.0.x, 1.1.x, and 3.x API families.

The former `remote-exec-daemon-xp` name described the original Windows XP-only
shape. Current behavior is documented here and in the repository root
`README.md`. The top-level `docs/` tree is historical implementation context,
not the live contract.

## At A Glance

| Area | Current behavior |
| --- | --- |
| Language level | C++11 on every supported build path. |
| Transport | HTTP/1.1 JSON over optional OpenSSL direct/reverse mTLS or explicit plain HTTP. |
| Authentication | Mutual TLS with optional certificate pinning, plus optional HTTP bearer auth. Bearer auth alone does not encrypt plain HTTP. |
| Exec | POSIX and Windows shell execution, live sessions, and stdin polling/writes. |
| PTY | POSIX PTY when available. GNU/MSVC Windows PTY depends on vendored `winpty`; Wine disables PTY capability reporting for GNU builds. |
| Patch | Codex-style `apply_patch` with complete syntax validation followed by sequential, non-transactional actions. A normal in-place update atomically replaces its target. `*** Environment ID: <id>` is retained as response metadata and never changes the selected target. |
| Images | PNG, JPEG, and WebP passthrough without resizing. |
| Transfers | Regular files, directory trees, and broker-built multi-source bundles. No transfer compression. |
| Port forwarding | v4 TCP/UDP tunnel support with daemon-local worker, socket, queue, and reconnect limits. |
| Hidden file tools | `read`, `write`, and `edit` are not implemented yet. |

TLS builds use `https://...` broker targets. Plain targets use `http://...`
plus `allow_insecure_http = true`:

```toml
[targets.builder-cpp]
base_url = "https://builder-cpp.example.com:8181"
expected_daemon_name = "builder-cpp"
tls_ca_pem = "/etc/remote-exec/ca.pem"
tls_client_cert_pem = "/etc/remote-exec/broker.pem"
tls_client_key_pem = "/etc/remote-exec/broker.key"

#[targets.builder-cpp.http_auth]
#bearer_token = "replace-me"
```

## Quick Start

Build and test a POSIX daemon:

```sh
make check-posix
```

Run it:

```sh
build/remote-exec-daemon-cpp config/daemon-cpp.example.ini
```

Run the BSD make POSIX path:

```sh
bmake check-posix
```

Run the common GNU Windows XP-compatible cross-build:

```sh
make prepare-openssl-xp OPENSSL_DEPS_DIR=/path/to/deps
make all-windows-xp OPENSSL_ROOT=/path/to/deps/openssl-1.1.1w
```

Run the MSVC native path from an x86 Visual Studio developer prompt:

```bat
nmake /f NMakefile check-msvc-native OPENSSL_ROOT=C:\path\to\openssl
```

Run the MSVC XP-compatible path from an x86 prompt with a `v141_xp`-capable
C++11 toolset, such as `vcvarsall.bat x86 -vcvars_ver=14.16`:

```bat
nmake /f NMakefile check-msvc-xp OPENSSL_ROOT=C:\path\to\openssl-xp
```

## Compatibility Matrix

The daemon builds as C++11 across all supported toolchains. In this repository,
"Windows XP-compatible" means a toolchain that can target Windows XP while
compiling the daemon as C++11; it does not imply a pre-C++11 language level.

| Path | Target family | Notes |
| --- | --- | --- |
| `make check-posix` | Native POSIX | Uses the platform `c++` driver. |
| `bmake check-posix` | Native POSIX through BSD make | POSIX-only; no Windows cross targets. |
| `make check-windows-xp` | GNU x86 Windows XP, Winsock 2 | Common legacy Windows cross-build. |
| `make check-windows-2000` | GNU x86 Windows 2000, Winsock 2 | NT-family Unicode path. |
| `make check-windows-x64` | GNU x64 NT-family Windows, Winsock 2 | NT-family only. |
| `make check-windows-nt3x-ws1` | GNU x86 NT 3.x API-floor, Winsock 1.1 | Keeps the 0x0400 Win32 API floor and disables winpty. |
| `make check-windows-nt4-ws1` | GNU x86 NT 4.0 API-floor, Winsock 1.1 | Unicode variant tested on Windows NT 3.51 and Windows NT 4.0. |
| `make check-windows-nt4-ws2` | GNU x86 NT 4.0 API-floor, Winsock 2 | Requires Winsock 2 at runtime. |
| `make check-windows-9x-ws1-ansi` | GNU x86 Windows 9x/Me ANSI, Winsock 1.1 | Tested on Windows 95 and Windows 98 SE. |
| `make check-windows-9x-ws2-ansi` | GNU x86 Windows 9x/Me ANSI, Winsock 2 | Older systems such as Windows 95 require Winsock 2 to be installed. |
| `nmake /f NMakefile check-msvc-native` | Host-native MSVC | CI runs the 32-bit native path on `windows-latest`. |
| `nmake /f NMakefile check-msvc-xp` | MSVC XP-compatible x86 | Requires an XP-capable C++11 toolset such as VS 2017 `v141_xp`. |

GNU Windows builds default to `WINDOWS_ARCH=x86` and Unicode Win32 APIs for the
historical legacy matrix. `WINDOWS_ARCH=x64` selects the x86_64 MinGW cross
compiler for NT-family builds. The Windows 9x/Me GNU aliases require x86 and
the ANSI Win32 API path.

## Build Guide

Build outputs are written under this directory's `build/` tree even when `make`
is invoked from the repository root. Incremental object files live under
`build/obj/`.

GNU make, BSD make, and NMAKE default to optimized builds (`-O2` or `/O2`).
Pass `DEBUG=1` to switch the relevant entry point to `-O0 -g` or
`/Od /Zi /DEBUG`.

NMAKE passes `/utf-8` so MSVC reads UTF-8 source files consistently across host
code pages. It enables `cl /MP` by default; override that compiler-side
parallelism with `MSVC_JOBS=<n>` or force serial compilation with
`MSVC_JOBS=1`.

Common targets:

| Task | Command |
| --- | --- |
| Build POSIX daemon | `make` or `make all-posix` |
| Build POSIX standalone patch CLI | `make apply-patch-posix` |
| Test POSIX daemon | `make check` or `make check-posix` |
| Stress POSIX lifecycle tests | `make STRESS_RUNS=10 STRESS_JOBS=8 stress-posix` |
| Build all GNU Windows variants | `make all-windows` plus the controls below |
| Test GNU Windows XP | `make check-windows-xp` |
| Test GNU Windows x64 | `make check-windows-x64` |
| Test BSD make POSIX path | `bmake check-posix` |
| Test MSVC native | `nmake /f NMakefile check-msvc-native` |
| Test MSVC XP-compatible | `nmake /f NMakefile check-msvc-xp` |

For direct standalone builds, use `make apply-patch-windows` with the same GNU
Windows controls as the daemon, or `nmake /f NMakefile apply-patch-msvc-native`
or `nmake /f NMakefile apply-patch-msvc-xp` for the selected MSVC target.

GNU Windows controls:

| Variable | Default | Purpose |
| --- | --- | --- |
| `WINDOWS_TOOLCHAIN` | `cross` on non-Windows, `native` on Windows GNU hosts | Selects MinGW cross compiler or host `g++`. Cross builds link `-static-libgcc -static-libstdc++`; non-Windows cross tests default `WINDOWS_TEST_RUNNER` to `wine`. |
| `WINDOWS_ARCH` | `x86` | Selects `i686-w64-mingw32-g++` or `x86_64-w64-mingw32-g++`. x64 is NT-family only. |
| `WINDOWS_WINVER` | `0x0501` | Selects the Win32 API floor. |
| `WINDOWS_WIN32_WINNT` | follows `WINDOWS_WINVER` | Overrides `_WIN32_WINNT` when needed. |
| `WINDOWS_FAMILY` | `nt` | Use `9x` for Windows 9x/Me aliases; this also defines `_WIN32_WINDOWS` and `_CHICAGO_`. |
| `WINDOWS_CHAR_API` | `unicode` | Use `ansi` for daemon-owned `A` Win32 file/process/path calls. ANSI builds reject UTF-8 paths or commands that are not representable in the active Windows ANSI code page. |
| `WINDOWS_WINSOCK_VERSION` | `2` | Use `1` for Winsock 1.1, IPv4-only, `wsock32`, and IPv6 rejection with `invalid_endpoint`. |
| `WINDOWS_WINPTY` | `auto` | Enables vendored `winpty` for NT 4.0, Windows 2000, and XP API-floor Unicode GNU builds. ANSI and NT 3.x auto-disable it. |
| `WINDOWS_TEST_RUNNER` | `wine` on non-Windows cross builds | Command used to run generated Windows test binaries. Leave empty to run directly. |

Useful GNU aliases:

| Alias family | Commands |
| --- | --- |
| NT 3.x Winsock 1.1 | `all-windows-nt3x-ws1`, `check-windows-nt3x-ws1` |
| NT 4.0 Winsock 1.1 | `all-windows-nt4-ws1`, `check-windows-nt4-ws1` |
| NT 4.0 Winsock 1.1 ANSI | `all-windows-nt4-ws1-ansi`, `check-windows-nt4-ws1-ansi` |
| NT 4.0 Winsock 2 | `all-windows-nt4-ws2`, `check-windows-nt4-ws2` |
| Windows 9x/Me ANSI | `all-windows-9x-ws1-ansi`, `check-windows-9x-ws1-ansi`, `all-windows-9x-ws2-ansi`, `check-windows-9x-ws2-ansi` |
| Windows 2000 | `all-windows-2000`, `check-windows-2000` |
| Windows XP | `all-windows-xp`, `check-windows-xp`, `all-windows-xp-ansi`, `check-windows-xp-ansi` |
| Windows x64 | `all-windows-x64`, `check-windows-x64` |
| Windows GNU native | `all-windows-native`, `check-windows-native` |

The XP, x64, XP ANSI, and native aliases resolve `TLS=auto` to OpenSSL and
therefore require a compatible OpenSSL installation. Pass `OPENSSL_ROOT`, or
use `TLS=off` when intentionally validating only the plain-HTTP build.

MSVC/NMAKE targets:

| Variant | Build | Check | Focused tests |
| --- | --- | --- | --- |
| Native | `nmake /f NMakefile all-msvc-native` | `nmake /f NMakefile check-msvc-native` | `test-msvc-native-console-output`, `test-msvc-native-session-store`, `test-msvc-native-transfer`, `test-msvc-native-server-routes-common`, `test-msvc-native-server-runtime`, `test-msvc-native-server-transport`, `test-msvc-native-connection-manager` |
| XP-compatible | `nmake /f NMakefile all-msvc-xp OPENSSL_ROOT=C:\path\to\openssl-xp` | `nmake /f NMakefile check-msvc-xp OPENSSL_ROOT=C:\path\to\openssl-xp` | `test-msvc-xp-console-output`, `test-msvc-xp-session-store`, `test-msvc-xp-transfer`, `test-msvc-xp-server-routes-common`, `test-msvc-xp-server-runtime`, `test-msvc-xp-server-transport`, `test-msvc-xp-connection-manager` |

`NMakefile` is intentionally separate from the GNU/BSD make entry points. It
uses the static C runtime (`/MT`), vendors the same `winpty` sources as GNU
Windows builds, stages `winpty-agent.exe` beside the daemon and test binaries,
and links XP targets as x86 console programs with a Windows XP minimum
subsystem version. NMAKE does not expose a Windows 2000 entry point.

Makefile layout:

- `GNUmakefile` is the GNU make public entry point.
- `Makefile` is the BSD make POSIX-only entry point.
- `mk/sources.mk` owns shared source lists.
- `mk/common.mk` owns shared GNU make helpers.
- `mk/windows-gnu.mk` owns the GNU Windows matrix.
- `mk/posix.mk` owns host-native POSIX rules.

## TLS Builds

`TLS=auto` is the default. On POSIX it probes the selected compiler, OpenSSL
headers, minimum 1.0.2 version, and link libraries; TLS is enabled only when the
probe succeeds. Windows XP and newer GNU/MSVC targets resolve auto to OpenSSL,
while Windows 2000, NT 4.0, NT 3.x, and 9x targets resolve auto to off. Explicit
`TLS=openssl` and `TLS=off` always override the automatic choice.

Use an installed OpenSSL:

```sh
make TLS=openssl OPENSSL_ROOT=/opt/openssl
```

Or prepare the checksum-pinned preferred OpenSSL 3.5.7 static dependency:

```sh
make prepare-openssl OPENSSL_JOBS=8
make TLS=openssl OPENSSL_ROOT="$PWD/build/deps/openssl-3.5.7"
```

`prepare-openssl` accepts `OPENSSL_SOURCE_CACHE_DIR` to reuse checksum-verified
source archives across build directories, `OPENSSL_ARCHIVE` for offline use,
`OPENSSL_CONFIGURE_TARGET` for cross builds, and
`OPENSSL_CONFIGURE_OPTIONS` for target-specific options. By default it builds
with `no-shared no-module no-tests`; use
`OPENSSL_BASE_CONFIGURE_OPTIONS` to replace that base set for an older OpenSSL
release that does not accept one of those options. For example, the tested
OpenSSL 1.1.x build is:

```sh
make prepare-openssl \
  OPENSSL_VERSION=1.1.1w \
  OPENSSL_SHA256=cf3098950cb4d853ad95c0841f1f9c6d3dc102dccfcacd521d93925208b76ac8 \
  OPENSSL_BASE_CONFIGURE_OPTIONS='no-shared no-tests'
```

External OpenSSL 1.x installs remain supported through `OPENSSL_ROOT`,
`OPENSSL_CPPFLAGS`, and `OPENSSL_LDLIBS`.

For an XP GNU cross-build, use a complete `i686-w64-mingw32-` tool prefix and
the checksum-pinned OpenSSL 1.1.1w dependency. It defaults to the
`i686-w64-mingw32-` compiler prefix; set `CROSS_COMPILE` when the selected
cross toolchain uses a different prefix:

```sh
make prepare-openssl-xp \
  OPENSSL_DEPS_DIR="$PWD/build/deps-mingw-xp" \
  OPENSSL_JOBS=8
make all-windows-xp \
  OPENSSL_ROOT="$PWD/build/deps-mingw-xp/openssl-1.1.1w"
```

The XP GNU path defaults to OpenSSL 1.1.1w, which has been built and exercised
on the Windows XP target. The `prepare-openssl-xp-1.0.x` alias explicitly
selects the legacy OpenSSL 1.0.2u API build. OpenSSL 1.0.2u is end-of-life and
should be used only when a legacy OpenSSL 1.0.x compatibility path is required.
OpenSSL 3.5.7 remains the preferred checksum-pinned dependency for modern
hosts.

GNU output/object names include the selected OpenSSL installation when TLS is
enabled, avoiding stale objects compiled against a different OpenSSL API. NMAKE
output/object names remain TLS-tagged to avoid mixing disabled and enabled builds.
Pre-XP Windows TLS combinations are best-effort and may fail in
the selected OpenSSL/toolchain build because they are not all tested.

## Run

Use `config/daemon-cpp.example.ini` as the starting config. The configured
`default_workdir` must already exist when the daemon starts.

| Build | Binary |
| --- | --- |
| POSIX | `build/remote-exec-daemon-cpp` |
| POSIX standalone patch CLI | `build/apply_patch` |
| GNU Windows XP/Winsock 2 | `build\remote-exec-daemon-cpp-xp-ws2-tls-openssl.exe` |
| GNU Windows x64 XP/Winsock 2 | `build\remote-exec-daemon-cpp-x64-xp-ws2-tls-openssl.exe` |
| GNU Windows 2000/Winsock 2 | `build\remote-exec-daemon-cpp-2000-ws2.exe` |
| GNU host-native Windows XP/Winsock 2 | `build\remote-exec-daemon-cpp-native-xp-ws2-tls-openssl.exe` |
| GNU Windows NT 3.x Winsock 1.1 | `build\remote-exec-daemon-cpp-nt3x-ws1.exe` |
| GNU Windows NT 4.0 Winsock 1.1 | `build\remote-exec-daemon-cpp-nt4-ws1.exe` |
| GNU Windows NT 4.0 Winsock 1.1 ANSI | `build\remote-exec-daemon-cpp-nt4-ws1-ansi.exe` |
| GNU Windows 9x/Me Winsock 1.1 ANSI | `build\remote-exec-daemon-cpp-9x-ws1-ansi.exe` |
| GNU Windows 9x/Me Winsock 2 ANSI | `build\remote-exec-daemon-cpp-9x-ws2-ansi.exe` |
| GNU Windows NT 4.0 Winsock 2 | `build\remote-exec-daemon-cpp-nt4-ws2.exe` |
| MSVC XP-compatible | `build\msvc-xp\remote-exec-daemon-cpp-xp-msvc-tls-openssl.exe` |

Every GNU Windows and MSVC daemon build also emits an `apply_patch-<variant>.exe`
alongside its daemon binary. The standalone tool reads a Codex-style patch from
standard input. `--help` prints built-in usage; `--help --help-file PATH`
prints help text from `PATH` instead. Without `--help`, `--help-file PATH` is
ignored. Errors name the failed action and include the underlying patch-engine
or filesystem error. If an execution error happens after earlier actions
complete, the tool prints `Partial success` with those completed actions before
exiting unsuccessfully. On Windows, these built-in success and error messages
are written through the Unicode console API when attached to a console, while
redirected output remains UTF-8.

Example:

```bat
build\remote-exec-daemon-cpp-xp-ws2-tls-openssl.exe config\daemon-cpp.example.ini
```

Logs go to `stderr`. Set `REMOTE_EXEC_LOG=debug` to raise the level, or use a
shared filter string such as:

```sh
REMOTE_EXEC_LOG=warn,remote_exec_daemon_cpp=debug
```

The old `remote_exec_daemon_xp=<level>` filter remains accepted as an alias.

## Config

The full example lives at `config/daemon-cpp.example.ini`. The core shape is:

```ini
target = builder-cpp
listen_host = 0.0.0.0
listen_port = 8181
default_workdir = /work

# Optional plain-HTTP bearer auth.
# http_auth_bearer_token = replace-me

# Optional shell policy.
# default_shell = /bin/bash
# allow_login_shell = true

# Optional request/session limits.
# max_request_header_bytes = 65536
# max_request_body_bytes = 536870912
# http_connection_idle_timeout_ms = 30000
# transfer_max_archive_bytes = 536870912
# transfer_max_entry_bytes = 536870912
# max_open_sessions = 64

# Optional forwarding limits.
# port_forward_max_worker_threads = 256
# port_forward_max_retained_sessions = 64
# port_forward_max_retained_listeners = 64
# port_forward_max_udp_binds = 64
# port_forward_max_active_tcp_streams = 1024
# port_forward_max_tunnel_queued_bytes = 8388608
# port_forward_tunnel_io_timeout_ms = 30000
# port_forward_connect_timeout_ms = 10000

# Optional per-operation yield-time limits.
# yield_time_exec_command_default_ms = 10000
# yield_time_exec_command_max_ms = 30000
# yield_time_exec_command_min_ms = 250
# yield_time_write_stdin_poll_default_ms = 5000
# yield_time_write_stdin_poll_max_ms = 300000
# yield_time_write_stdin_poll_min_ms = 5000
# yield_time_write_stdin_input_default_ms = 250
# yield_time_write_stdin_input_max_ms = 30000
# yield_time_write_stdin_input_min_ms = 250

# Optional static path sandbox. Values are semicolon-separated path lists.
# sandbox_exec_cwd_allow = /work
# sandbox_exec_cwd_deny = /work/private
# sandbox_read_allow = /work;/assets
# sandbox_read_deny = /work/.git;/assets/secrets
# sandbox_write_allow = /work
# sandbox_write_deny = /work/.git;/work/readonly
```

C++-specific limit notes:

- `max_open_sessions` intentionally matches the Rust daemon default of 64.
- `max_request_header_bytes` and `max_request_body_bytes` exist because this
  daemon owns a handwritten HTTP parser.
- `max_request_body_bytes` applies to buffered HTTP request bodies. Streaming
  transfer imports are bounded by `transfer_max_archive_bytes` and
  `transfer_max_entry_bytes` instead.
- `port_forward_max_worker_threads` and `port_forward_tunnel_io_timeout_ms`
  exist because forwarding uses blocking socket workers.

Reverse mode is exclusive and supports TLS in an OpenSSL-enabled build or
explicit plain HTTP:

```ini
#connection_mode=reverse
#reverse_broker_host=broker.example.com
#reverse_broker_port=9555
#reverse_transport=tls
#reverse_tls_cert_pem=/etc/remote-exec/daemon.pem
#reverse_tls_key_pem=/etc/remote-exec/daemon.key
#reverse_tls_ca_pem=/etc/remote-exec/ca.pem
#reverse_tls_server_name=broker.example.com
#reverse_bearer_token=replace-me
#reverse_min_idle_connections=4
#reverse_max_connections=128
#reverse_reconnect_ms=1000
```

Sandbox rules mirror the Rust daemon's static allow/deny model:

- `sandbox_exec_cwd_*` applies to the resolved starting `cwd` for
  `exec_command`.
- `sandbox_read_*` applies to transfer export source paths.
- `sandbox_write_*` applies to transfer import destinations, transfer path-info
  destination probes, and resolved `apply_patch` write targets.
- Empty or omitted `allow` lists allow all paths for that access class; `deny`
  entries override allow membership.
- POSIX roots are canonicalized through existing ancestors, so symlinks in
  configured roots or requested paths cannot bypass boundary checks.
- Windows roots use Windows-style normalization and case-insensitive matching.

## Behavior Notes

### Shells And Encoding

- POSIX default shell selection follows the Rust daemon policy: configured
  `default_shell`, then `SHELL`, passwd shell, `bash`, and `/bin/sh`.
- POSIX exec uses `shell -c <cmd>` or `shell -l -c <cmd>` for login shells.
- POSIX child processes currently force `LC_ALL=C.UTF-8` and `LANG=C.UTF-8`.
- Windows exec supports `cmd.exe` and `command.com`. `cmd.exe` uses `/C` and
  adds `/D` when `login=false`; `command.com` uses `/C`.
- Windows command stdout/stderr bytes are decoded as OEM code page first, then
  ANSI code page, then UTF-8 replacement decoding. This covers legacy console
  encodings such as GBK, Shift-JIS, and Big5 when the OS provides those code
  page tables.
- GNU ANSI Win32 API builds convert daemon-controlled UTF-8 paths and commands
  through the active Windows ANSI code page before calling `A` Win32 APIs, and
  reject unrepresentable input instead of silently replacing it.

### Exec And PTY

- Non-TTY exec output merges `stdout` and `stderr` through one pipe, preserving
  emitted order in the returned `output` field.
- POSIX non-TTY exec starts child stdin at `/dev/null`, matching the Rust
  daemon's closed-stdin behavior. Use `tty=true` for interactive POSIX
  commands that need later `write_stdin` input.
- Windows C++ non-TTY exec keeps pipe-backed stdin open to preserve the
  original XP daemon behavior.
- POSIX builds support `write_stdin.pty_size` for live `tty=true` sessions via
  `TIOCSWINSZ`.
- Windows GNU/MSVC builds with `winpty` enabled forward PTY resize requests
  through `winpty`.
- Builds without `winpty` reject `tty=true` and return the same typed
  unsupported-session error path for resize requests.

### Transfers

- `transfer_files` supports regular files, directory trees, and broker-built
  multi-source bundles.
- Export-side `exclude` patterns match paths relative to each source root, use
  `/` as the logical separator on all platforms, and support `*`, `?`, `**`,
  `[abc]`, `[a-z]`, `[!abc]`, `[!a-c]`, `[^abc]`, and `[^a-c]`.
- Excluded matches are silent, excluded directories are pruned recursively, and
  single-file sources ignore `exclude` in v1.
- HTTP transfer imports and exports stream archive bodies instead of staging
  the full tar payload in memory.
- Transfer imports support `fail`, `merge`, and `replace` overwrite modes.
  `merge` overlays compatible existing destinations without deleting unrelated
  entries. `replace` refreshes a single file or directory destination; for
  multi-source bundle imports, it replaces only incoming top-level entries.
- Transfer imports are not transactional; failed imports can leave partial
  destination changes.
- POSIX exports skip unsupported special entries in directory trees and report
  warnings.
- POSIX symlink modes support preserving, following, or skipping symlinks.
- Windows C++ builds skip symlink entries when preservation is unavailable;
  follow mode copies regular-file and directory targets when the platform
  exposes them.
- Transfer payloads use GNU tar for files and directories. Single-file
  transfers use the fixed archive entry `.remote-exec-file`.
- Transfer warnings use `.remote-exec-transfer-summary.json`, which is consumed
  during import and is not extracted.
- Unsupported archive entries remain rejected: hard links, special files unless
  skipped during export, sparse entries, and malformed paths.

### Port Forwarding

- The daemon implements the same HTTP/1.1 Upgrade v4 tunnel used by broker
  `forward_ports`.
- TCP listeners/connectors, full-duplex UDP datagram sockets, non-loopback
  listen binds, and bare-port normalization are supported.
- The older lease-renewed port-forward routes are not exposed.
- The v4 frame numbers for `ForwardRecovering` and `ForwardRecovered` are
  reserved for compatibility with the Rust protocol table. Current public
  recovery state is reported through broker-owned `forward_ports list` state.
- When only broker-daemon transport drops and the daemon stays alive, the
  broker may recover the forward itself plus future TCP accepts or UDP datagrams
  on the listen side.
- Active TCP streams and UDP per-peer connector state are not preserved across
  reconnect.
- Each active forwarded TCP stream uses separate read and write workers.
- Retained sessions, retained listeners, UDP binds, active TCP streams, and
  outbound queued tunnel bytes are enforced daemon-wide.
- Tunnel socket reads/writes are bounded by
  `port_forward_tunnel_io_timeout_ms`; outbound TCP connects are bounded by
  `port_forward_connect_timeout_ms`.
- Per-stream TCP connect failures close only that accepted TCP stream and leave
  the parent forward open.
- Broker restart drops broker-owned `forward_id` mappings. Daemon restart
  destroys daemon-local forward state.
- If the broker disappears without reconnecting, the daemon reclaims detached
  listeners and UDP sockets after the reconnect grace window expires.

### Images And Patches

- `view_image` returns PNG, JPEG, and WebP without resizing.
- `view_image.detail` is accepted for compatibility but has no effect.
- `apply_patch` parses the complete patch before writing, so malformed syntax
  makes no changes.
- Valid actions run sequentially and are non-transactional across actions;
  later filesystem, sandbox, or hunk-match failures can leave earlier actions
  applied. A normal in-place update atomically replaces its target.

## Debugging And Tests

Keep normal test output quiet with `REMOTE_EXEC_LOG=off`, and raise logs only
when chasing lifecycle-sensitive failures:

```sh
REMOTE_EXEC_LOG=debug make test-host-server-streaming
REMOTE_EXEC_LOG=debug make test-host-session-store
```

Lifecycle debug logs include tunnel session IDs, generations, close modes,
retained resource kinds, worker start/finish events, writer shutdown, and
tunnel read timeout decisions.

Use stress targets for load-sensitive POSIX flakes:

```sh
make STRESS_RUNS=10 STRESS_JOBS=8 stress-posix
bmake STRESS_RUNS=10 STRESS_JOBS=8 stress-posix
```

Attach a debugger to a stuck process:

```sh
gdb -p <pid>
```

Run a test binary directly under a debugger:

```sh
REMOTE_EXEC_LOG=debug gdb --args build/test_server_streaming
REMOTE_EXEC_LOG=debug gdb --args build/test_session_store
```

For Windows-shared lifecycle changes, run representative GNU Windows paths. On
non-Windows hosts these use Wine when `WINDOWS_TEST_RUNNER` is unset and Wine is
available:

```sh
make check-windows-xp
make check-windows-x64
make check-windows-2000
make check-windows-nt4-ws1
make check-windows-nt4-ws2
```

Focused host-native tests:

```sh
make test-host-patch
make test-host-transfer
make test-host-config
make test-host-http-request
make test-host-server-transport
make test-host-session-store
make test-host-connection-manager
make test-host-tls-transport
make test-host-server-runtime
make test-host-server-routes
make test-host-server-streaming
make test-host-sandbox
```

Use the same target names with `bmake ...` for the BSD make path.

Runtime coverage note:

- POSIX C++ daemon runtime tests run on Unix.
- GNU Windows binaries and tests, including x64, run under Wine on Linux.
- MSVC native binaries and tests run on Windows.
- MSVC XP-compatible binaries run natively on Windows when the XP-capable
  toolset is available.
- The Windows Rust test job first builds
  `build\msvc-native\remote-exec-daemon-cpp-msvc.exe`, then sets
  `REMOTE_EXEC_CPP_DAEMON` to that binary for Rust integration tests.

## Internal Boundaries

The C++ daemon is split by ownership rather than by build target:

| Area | Owns |
| --- | --- |
| `platform/` | Raw OS primitives, compatibility fallbacks, fd/handle/socket wrappers, EINTR retry policy, close-on-exec and inheritance behavior, wakeup helpers, monotonic waits, PTY probes, process waits, and signal installation. |
| `runtime/` | Daemon lifecycle, accept/maintenance workers, connection accounting, shutdown propagation, and daemon-owned thread joins. |
| `http/` | Request parsing, body streams, response rendering, upgrade mechanics, and transport lifetime. |
| `rpc/` | Route dispatch, request validation, typed error translation, and capability response shaping. |
| `policy/` | Path comparison and sandbox evaluation. |
| `exec/`, `transfer/`, `port_forward/` | Feature behavior built on the lower layers. |

Feature code should use existing `platform/` wrappers instead of raw blocking OS
APIs such as `read`, `write`, `recv`, `send`, `accept`, `connect`, `poll`,
`select`, `waitpid`, `pthread_cond_*`, `fcntl`, `open`, `pipe`, `sigaction`,
`kill`, `execve`, `fork`, `bind`, `listen`, `setsockopt`, `getsockname`,
`ioctl`, filesystem mutation calls, or Win32 wait/process/socket primitives. If
a new raw OS call is required, add the wrapper or fallback in `platform/`
first.

Feature code should also avoid hand-rolled elapsed/remaining timeout arithmetic
for blocking waits. Use `platform/deadline.h` so timeout saturation, bounded
wait slices, and POSIX `poll` retry behavior stay in one place.

The POSIX design intentionally does not use `openat`. Path authorization must
therefore be complete and consistent for every materialized path, but it should
not be described as race-free filesystem security. Recursive transfer import,
export, replacement, and cleanup code must authorize the concrete path it is
about to open, remove, create, or rename.

Port forwarding has stricter ownership rules because it is reconnect-aware:

- `PortTunnelService` owns global limits and the retained session map.
- `PortTunnelSession` owns retained session state and retained resources.
- `PortTunnelConnection` owns one upgraded tunnel connection and
  connection-local streams.
- Resource objects own their sockets and budget leases.
- Resource objects move through explicit `open`, `closing`, and `closed`
  states. Callers should ask the resource for its state instead of carrying
  separate closed flags.
- Close work should be returned to callers and performed outside unrelated
  locks.

## Limitations

- `TLS=auto` enables usable OpenSSL on POSIX and enables it for Windows XP and
  newer targets; pre-XP Windows targets default to TLS off.
- HTTP/1.1 only. Sequential requests may reuse a persistent connection, but
  HTTP pipelining is not supported.
- OpenSSL older than 1.0.2 is rejected; TLS 1.0 and 1.1 are disabled.
- No transfer compression support.
- No default-hidden `read`, `write`, or `edit` tool support yet.
- `view_image` preserves PNG, JPEG, and WebP without resizing; `detail` has no
  effect.
- POSIX PTY support depends on host PTY allocation.
- Windows PTY support depends on `winpty` being enabled and usable at runtime.
- GNU NT 3.x builds and GNU ANSI API builds where `WINDOWS_WINPTY=auto`
  resolves to disabled have no PTY support.
- Windows runtime support is strongest on Unicode NT-family Windows.
- GNU x64 builds are NT-family only; Windows 9x/Me aliases require
  `WINDOWS_ARCH=x86`.
- The GNU Winsock 1.1 Unicode variant has been tested on Windows NT 3.51 and
  Windows NT 4.0.
- The GNU ANSI API path has been tested with Winsock 1.1 and Winsock 2 on
  Windows 95 and Windows 98 SE.
- `transfer_files` is not transactional and can leave partial destination
  changes after failure.
- Broker-owned `forward_id` values do not persist across broker restart.
- Transient broker-daemon transport drops preserve only the forward itself plus
  future listen-side TCP accepts or UDP datagrams; active TCP streams and UDP
  per-peer connector state are lost.
- TLS builds use `https://...` broker targets; plain targets use `http://...`
  plus `allow_insecure_http = true`.
- Optional bearer auth can additionally authenticate requests; on plain HTTP it
  still does not encrypt traffic.
