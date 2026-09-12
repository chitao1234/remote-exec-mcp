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
| Hidden file tools | `read`, `write`, and `edit`, matching the Rust daemon's semantics and wire error codes. |
| Text encoding | Optional. When built in and enabled in config, text tools decode and re-encode legacy encodings instead of requiring UTF-8. |

Windows commands are detached from the daemon console so a command waiting
for console input does not block the daemon. On Windows 9x/Me, exited commands
finish after a bounded drain of final output even when their redirected pipes
do not report EOF.

Exec calls and stdin polls keep waiting within their remaining `yield_time_ms`
budget when a process has exited but final output has not finished draining.
They report completion once both process exit and output completion are observed.

OpenSSL- or LibreSSL-backed builds and POSIX BoringSSL-backed builds support
direct and reverse mutual TLS. Plain HTTP is available only when explicitly
selected. Bearer authentication can authenticate plain HTTP requests, but it
does not encrypt them.

## Text encoding

By default the file tools and `apply_patch` require UTF-8, exactly as before.
Setting `experimental_apply_patch_target_encoding_autodetect = true` lets them
decode a target file's existing encoding and write the result back in that same
encoding, so a read/modify/write cycle leaves the surrounding bytes alone.

Detection covers UTF-8 with or without a byte-order mark, UTF-16LE/BE with or
without a byte-order mark, and Shift_JIS, EUC-JP, GB18030 (which also decodes
GBK and GB2312), Big5, EUC-KR, and the single-byte code pages Windows-1250
through Windows-1258, ISO-8859-2 through ISO-8859-16, KOI8-R, KOI8-U, Macintosh,
and IBM866.

Two properties are worth knowing before enabling it:

- Detection is statistical, so a very short file may be identified incorrectly.
  Every decoded file is re-encoded and compared against the original bytes
  before it is accepted, so a wrong guess is reported as a decode failure rather
  than silently rewriting the file. Files that are clearly binary are refused on
  the same path.
- An edit that introduces a character the detected encoding cannot represent
  (for example an emoji in a Shift_JIS file) fails with `file_write_failed`
  instead of substituting a replacement character.

The feature can also be left out of the binary entirely. Build with
`TEXT_ENCODING=off` to omit the detector and the conversion tables:

```sh
make check-posix TEXT_ENCODING=off
```

Without it, non-UTF-8 files report a decode failure and the setting has no
effect. `TEXT_ENCODING` accepts `auto` (the default, feature enabled), `on`, and
`off`, and applies to the GNU, BSD, and NMAKE build paths.

## Quick start

From this directory, build and test the native POSIX path:

```sh
make check-posix
```

On macOS these commands work with the GNU Make 3.81 and Apple Clang supplied
by the Xcode Command Line Tools. Feature probes respect the compiler's macOS
deployment target, so an SDK declaration for a newer API does not enable calls
that are unavailable on the target OS. Set `MACOSX_DEPLOYMENT_TARGET` when
building for an older macOS release. Target metadata reports `macos`, with
`aarch64` for native Apple Silicon builds and `x86_64` for Intel builds.

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

GNU make, BSD make, and NMAKE embed the Git revision and UTC build date in
`daemon_version`, reported by health, target-info, and the broker's target
metadata. The format matches Rust: `0.2.0+g<short-commit>.<YYYYMMDD>`. Git uses
at least 12 hexadecimal characters for the commit. Builds without Git metadata
use `0.2.0+<YYYYMMDD>`. Incremental C++ builds refresh this metadata when HEAD
or the UTC date changes; uncommitted changes do not add a `dirty` marker.
NMAKE uses Windows PowerShell to generate the build metadata.

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
- request, session, transfer, forwarding, yield-time limits, and
  `stdin_write_timeout_ms` (30 seconds by default);
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

## Separate build and test machines

The build targets can stage a relocatable test-bundle directory without
running target binaries. This lets a compiler machine produce the daemon,
`apply_patch`, the test executables, runtime companions such as
`winpty-agent.exe`, and all test fixtures, then lets a different compatible
machine run that exact payload without a source checkout, Make/NMAKE, or a C++
compiler.

Create a bundle on the build machine:

```sh
# Native POSIX, with GNU make or BSD make.
make test-bundle-posix
bmake test-bundle-posix

# Cross-compiled GNU Windows variants; no Wine execution occurs here.
make test-bundle-windows-xp TLS=off
make test-bundle-windows-x64 TLS=off
```

For MSVC, use the equivalent target from the matching developer prompt:

```bat
nmake /f NMakefile test-bundle-msvc-native TLS=off
nmake /f NMakefile test-bundle-msvc-xp TLS=off
nmake /f NMakefile test-bundle-msvc-xp-x64 TLS=off
```

Each command prints its bundle directory under `build/test-bundles/`. Copy that
whole directory to the execution machine, preserving its layout and executable
permissions. A tar archive is a convenient way to preserve POSIX permissions.
On the execution machine, run all tests, list them, or select individual tests:

```sh
./run-tests
./run-tests --list
./run-tests transfer server-streaming
```

```bat
run-tests.exe
run-tests.exe --list
run-tests.exe transfer server-streaming
```

Set `TEST_BUNDLE_DIR` to stage directly into an artifact-collection directory:

```sh
make test-bundle-windows-xp TLS=off TEST_BUNDLE_DIR=/staging/cpp-xp-tests
```

```bat
nmake /f NMakefile test-bundle-msvc-xp TLS=off TEST_BUNDLE_DIR=C:\staging\cpp-xp-tests
```

Use a fresh or variant-specific destination when overriding the directory; a
custom destination is not removed by `make clean`.

The runner changes to its own directory before reading `tests.txt`, so the
bundle can be unpacked anywhere and launched by path from another working
directory. POSIX bundles use `sh` for the `apply-patch-cli` test. The execution
machine must match the built OS/architecture compatibility target and provide
any dynamically linked libraries selected by custom compiler or TLS flags.
The repository's prepared OpenSSL paths are static by default.

The existing `check-posix`, `check-windows-*`, and `check-msvc-*` targets remain
the local build-and-test entry points and also validate the corresponding
bundle workflow. Existing focused test targets remain available for
source-tree development.

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

- Windows `default_shell`, `COMSPEC`, and per-command `shell` accept arbitrary
  executable names and paths. The operating system resolves or rejects the
  executable at launch. Cmd, COMMAND.COM, PowerShell, and Bash retain their
  argument conventions; other executables receive `/C` and the command as one
  argument.
- POSIX non-PTY command output merges stdout and stderr in emitted order. Use
  `tty=true` for interactive commands that need later stdin writes.
- POSIX login shells run with `-l`. On macOS, `bash` and the `sh` shipped as
  Bash load their profiles this way for noninteractive commands.
- POSIX child environments drop inherited `LANG` and every `LC_*` override,
  then apply the locale plan. macOS uses `LANG=C` with a UTF-8 `LC_CTYPE`
  because `C.UTF-8` is absent there; other systems prefer `C.UTF-8` when it
  exists.
- Windows non-PTY commands run detached from the daemon's console while their
  standard streams remain connected to daemon-owned pipes. A stdin pipe write
  that exceeds `stdin_write_timeout_ms` terminates and retires the session so
  the request and daemon shutdown cannot remain blocked indefinitely.
- Console detachment does not provide a PTY. Legacy Windows builds without a
  usable WinPTY continue to advertise `supports_pty = false`.
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
Wine). The `test-bundle-windows-*` targets only build and stage the payload;
they do not invoke Wine. Run the MSVC checks from the appropriate Visual Studio
developer prompt.

For the complete repository quality gate and cross-crate integration tests, see
the root [README](../../README.md). The Makefiles are the source of truth for
available targets and compatibility aliases; the example config is the source
of truth for all daemon settings.
