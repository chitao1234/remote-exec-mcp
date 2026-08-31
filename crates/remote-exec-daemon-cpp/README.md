# remote-exec-daemon-cpp

`remote-exec-daemon-cpp` is the standalone C++11 daemon for
`remote-exec-mcp`. Use it when a smaller daemon is useful or when the legacy
Windows build matrix is required. It speaks the broker-daemon HTTP/1.1 JSON
protocol and supports POSIX, GNU Windows, and MSVC builds.

The repository root [README](../../README.md) documents the complete
`remote-exec-mcp` system. This file covers the C++ daemon's build and runtime
workflow.

## Capabilities

| Area | Support |
| --- | --- |
| Commands | Shell execution, live sessions, and stdin polling/writes. |
| PTY | POSIX PTYs when available; Windows PTYs when vendored `winpty` is enabled and usable. |
| Patching | Codex-style `apply_patch`; the complete patch is parsed before any write, then valid actions run sequentially. Actions are not transactional across the whole patch. |
| Images | PNG, JPEG, and WebP passthrough without resizing. `detail` is accepted for protocol compatibility and has no effect. |
| Transfers | Regular files, directory trees, and broker-built multi-source bundles. Imports and exports stream tar bodies; transfer compression is not implemented. |
| Port forwarding | v4 TCP/UDP upgrade tunnels, including reconnect handling and resource limits. |
| Hidden file tools | `read`, `write`, and `edit` are not implemented by this daemon. |

OpenSSL- or LibreSSL-backed builds and POSIX BoringSSL-backed builds support
direct and reverse mutual TLS. Plain HTTP is available only when explicitly
selected. Bearer authentication can authenticate plain HTTP requests, but it
does not encrypt them.

## Quick start

From this directory, build and test the native POSIX path:

```sh
make check-posix
```

Run the daemon with the example configuration:

```sh
build/remote-exec-daemon-cpp config/daemon-cpp.example.ini
```

The configured `default_workdir` must already exist when the daemon starts.
The example file is the canonical configuration reference; copy it and set
the target name, listener, work directory, and transport credentials.

From the repository root, the equivalent commands are:

```sh
make -C crates/remote-exec-daemon-cpp check-posix
crates/remote-exec-daemon-cpp/build/remote-exec-daemon-cpp \
  crates/remote-exec-daemon-cpp/config/daemon-cpp.example.ini
```

## Configuration

The daemon accepts an INI-style file. The minimum direct-listener shape is:

```ini
target = builder-cpp
listen_host = 0.0.0.0
listen_port = 8181
default_workdir = /work

# Optional policy switches; both default to true.
# allow_exec = false
# allow_apply_patch = false

# Optional plain-HTTP bearer authentication.
# http_auth_bearer_token = replace-me
```

For TLS, set `transport = tls` and provide the daemon certificate, private key,
and CA paths. The broker target must use `https://`, with matching client
credentials. For plain HTTP, leave TLS disabled and set
`allow_insecure_http = true` on the broker target.

Reverse mode is exclusive with a direct listener. Set `connection_mode=reverse`
and the broker connection, transport, and authentication settings shown in
`config/daemon-cpp.example.ini`. Reverse TLS requires a TLS-enabled build;
plain reverse mode requires `reverse_bearer_token`.

Optional settings cover:

- `default_shell`, `allow_login_shell`, and Windows `windows_posix_root` for
  command execution and single-slash POSIX path translation;
- request, session, transfer, forwarding, and yield-time limits;
- static `sandbox_*_allow` and `sandbox_*_deny` path rules.

Omitted or empty allow lists permit all paths for that access class; deny rules
override them. The sandbox applies to resolved operation paths and is not a
general filesystem race-safety guarantee.

## Build paths

All supported builds use C++11. The default GNU make target is native POSIX.
BSD make provides the POSIX path; NMAKE provides native and XP-compatible MSVC
paths.

| Path | Build/check command | Notes |
| --- | --- | --- |
| Native POSIX | `make check-posix` | Uses the host C++ compiler. |
| BSD make POSIX | `bmake check-posix` | POSIX-only entry point. |
| GNU Windows XP (x86) | `make check-windows-xp` | Winsock 2, Unicode Win32 APIs. |
| GNU Windows XP (x64) | `make check-windows-x64` | NT-family x64 target. |
| GNU Windows 2000 | `make check-windows-2000` | Winsock 2, Unicode Win32 APIs. |
| GNU NT 3.x/NT 4.0 | `make check-windows-nt3x-ws1` or `make check-windows-nt4-ws1` | Winsock 1.1 compatibility paths. |
| GNU Windows 9x/Me | `make check-windows-9x-ws1-ansi` or `make check-windows-9x-ws2-ansi` | x86 ANSI Win32 paths. |
| MSVC native | `nmake /f NMakefile check-msvc-native` | Run from a Visual Studio developer prompt. |
| MSVC XP-compatible x86 | `nmake /f NMakefile check-msvc-xp` | Requires an XP-capable C++11 toolset, such as `v141_xp`. |
| MSVC XP-compatible x64 | `nmake /f NMakefile check-msvc-xp-x64` | Run from an x64 developer prompt. |

GNU Windows aliases also exist for NT 4.0 Winsock 2, ANSI NT 4.0, XP ANSI,
Windows-native GNU, and the corresponding `all-*` build targets. Set
`WINDOWS_ARCH`, `WINDOWS_WINVER`, `WINDOWS_FAMILY`,
`WINDOWS_CHAR_API`, or `WINDOWS_WINSOCK_VERSION` when selecting a custom GNU
compatibility combination. `WINDOWS_TEST_RUNNER=wine` is the default for GNU
cross-tests on non-Windows hosts.

Build outputs are under `build/`. GNU binaries are named for their selected
Windows/TLS variant. MSVC binaries are under `build/msvc-native`,
`build/msvc-xp`, or `build/msvc-xp-x64`. Each daemon build also produces the
standalone `apply_patch` CLI for that variant.

## TLS builds

`TLS=auto` is the default:

- POSIX enables TLS when OpenSSL 1.0.2 or newer, LibreSSL 2.7.1 or newer, or
  BoringSSL is detected, and uses the detected library automatically;
- Windows XP and newer GNU/MSVC paths resolve `auto` to OpenSSL;
- older Windows compatibility paths resolve `auto` to TLS off.

Use `TLS=off` when plain HTTP is intentional. For OpenSSL-backed TLS builds
without a system OpenSSL installation, prepare the matching dependency and pass
its install directory:

```sh
make prepare-openssl
make TLS=openssl OPENSSL_ROOT="$PWD/build/deps/openssl-3.5.7"

make prepare-openssl-xp OPENSSL_DEPS_DIR="$PWD/build/deps-mingw-xp"
make all-windows-xp \
  OPENSSL_ROOT="$PWD/build/deps-mingw-xp/openssl-1.1.1w"
```

Use an existing OpenSSL, LibreSSL, or BoringSSL installation instead with
`OPENSSL_ROOT`, `OPENSSL_CPPFLAGS`, and `OPENSSL_LDLIBS`. The `TLS=openssl` value
is retained as the compatibility-mode name for any supported library. TLS
negotiates TLS 1.2 or newer.
OpenSSL 1.0.x remains available when a legacy compatibility path requires it.

## Runtime notes

- POSIX non-PTY command output merges stdout and stderr in emitted order. Use
  `tty=true` for interactive commands that need later stdin writes.
- Transfers and patch actions are non-transactional: a later failure can leave
  earlier filesystem changes in place. Transfer overwrite modes are `fail`,
  `merge`, and `replace`.
- A broker-daemon transport drop can preserve the forward and future listen-side
  traffic after reconnect. Active TCP streams and UDP peer connector state are
  not preserved.
- Broker-owned `session_id` and `forward_id` values do not survive broker
  restart. Daemon restart drops daemon-local sessions and forwards.
- Logs go to stderr. Set `REMOTE_EXEC_LOG=debug` while investigating a failure.

## Testing

Run the focused POSIX check before broader repository checks:

```sh
make check-posix
```

Useful additional checks are:

```sh
make check-windows-xp
make check-windows-x64
make test-host-transfer
make test-host-server-streaming
bmake check-posix
```

On non-Windows hosts, GNU Windows tests use `WINDOWS_TEST_RUNNER` (normally
Wine). Run the MSVC checks from the appropriate Visual Studio developer prompt.

For the complete repository quality gate and cross-crate integration tests, see
the root [README](../../README.md). The Makefiles are the source of truth for
available targets and compatibility aliases; the example config is the source
of truth for all daemon settings.
