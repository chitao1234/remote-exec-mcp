# C++ Windows Unicode Boundary Cleanup

## Goal

Clean up the C++ daemon's Windows text boundary so that internal code remains
UTF-8, Windows-facing calls use wide Win32 APIs or wide CRT entry points, and
the Windows XP MinGW + Wine gate remains green throughout the change.

## Verified Findings

- Production code still has one direct ANSI Win32 API call:
  - `crates/remote-exec-daemon-cpp/src/platform.cpp`
  - `GetComputerNameA(...)`
- Production code still has one narrow Windows environment read on a path-like
  value:
  - `crates/remote-exec-daemon-cpp/src/shell_policy.cpp`
  - `std::getenv("COMSPEC")`
- Windows test support still has narrow environment reads for the temp root:
  - `crates/remote-exec-daemon-cpp/tests/test_filesystem.h`
  - `std::getenv("TEMP")`
  - `std::getenv("TMP")`
- Windows build modes still do not define `UNICODE` / `_UNICODE`:
  - `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
  - `crates/remote-exec-daemon-cpp/mk/windows-native.mk`
  - `crates/remote-exec-daemon-cpp/NMakefile`
- The current vendored `httplib.h` Windows file-path path already uses explicit
  UTF-8 to UTF-16 conversion and `CreateFileW`; it is not the primary cleanup
  target in this pass.
- The default XP MinGW + Wine gate passes after repairing Windows test source
  wiring in commit `1057f67`.
- A full `UNICODE` / `_UNICODE` XP probe gets deep into the suite and then
  fails in `test_transfer-xp` with a `RemoveDirectoryW` cleanup error under
  Wine. Treat that as a hardening follow-up to resolve before making the macro
  flip the default.

## Requirements And Constraints

- Preserve the daemon's public wire format and public behavior.
- Keep Windows-facing text/path/environment boundaries on native wide Win32 or
  wide CRT calls.
- Keep internal string handling UTF-8-based.
- Do not introduce a broad new abstraction layer. A small Windows text boundary
  helper is acceptable if it replaces duplicated conversions and clarifies the
  call boundary.
- Keep GNU make, Windows-native make, and NMake aligned for Windows build-mode
  behavior.
- Use the MinGW + Wine XP gate as the required verification path.
- Do not broaden this pass into unrelated socket or transport redesign work.

## Architecture

Use a narrow Windows boundary model:

- Internal daemon code continues to pass UTF-8 `std::string` values.
- A small Windows-only helper owns UTF-8 <-> UTF-16 conversion plus selected
  Win32 text queries that must return UTF-8 into the core.
- Windows code calls explicit `...W` APIs or wide CRT entry points at the
  boundary.
- Build flags define `UNICODE` / `_UNICODE` so unsuffixed Win32 text APIs
  cannot silently bind to ANSI in future edits.

Likely helper responsibilities:

- UTF-8 to UTF-16 conversion
- UTF-16 to UTF-8 conversion
- environment variable lookup by wide name, returning UTF-8
- hostname lookup via `GetComputerNameW`
- temp-directory lookup via `GetTempPathW`

This should stay focused. The helper exists to narrow the Windows text
boundary, not to become a general platform subsystem.

## File And Component Shape

- `crates/remote-exec-daemon-cpp/src/platform.cpp`
  - switch hostname lookup to the wide path
- `crates/remote-exec-daemon-cpp/src/shell_policy.cpp`
  - stop using narrow `COMSPEC` reads on Windows
- `crates/remote-exec-daemon-cpp/tests/test_filesystem.h`
  - stop using narrow `TEMP` / `TMP` reads on Windows
- `crates/remote-exec-daemon-cpp/mk/windows-xp.mk`
  - add `UNICODE` / `_UNICODE`
- `crates/remote-exec-daemon-cpp/mk/windows-native.mk`
  - add `UNICODE` / `_UNICODE`
- `crates/remote-exec-daemon-cpp/NMakefile`
  - add `UNICODE` / `_UNICODE`
- `crates/remote-exec-daemon-cpp/src/` plus `include/`
  - add or extend the small Windows text helper if needed
- Windows-capable tests under `crates/remote-exec-daemon-cpp/tests/`
  - add focused regression coverage for Unicode-sensitive environment and temp
    path behavior

## Task Breakdown

### Task 1: Establish The Windows Text Boundary Helper

- Add a small Windows-only helper for UTF-8/UTF-16 conversion and selected
  Win32 text queries.
- Reuse existing conversion logic instead of keeping more one-off copies.
- Keep the helper intentionally small and Windows-only.

### Task 2: Remove Remaining Narrow Production Boundaries

- Replace `GetComputerNameA` with a wide hostname lookup.
- Replace Windows `COMSPEC` retrieval via `std::getenv` with a wide
  environment lookup returning UTF-8.
- Re-audit touched production files to ensure no new unsuffixed or ANSI Win32
  text calls remain in that slice.

### Task 3: Fix Windows Test Harness Unicode Inputs

- Replace Windows test temp-root lookup via `TEMP` / `TMP` narrow reads with a
  wide Win32 temp-directory lookup.
- Add Windows-capable regression coverage for Unicode-sensitive inputs:
  - default shell resolution from a wide `COMSPEC`
  - temp-root creation/cleanup under a Unicode path component

### Task 4: Harden Build Mode And Close The Probe Gap

- Add `UNICODE` / `_UNICODE` to Windows GNU make and NMake build flags.
- Run the full XP MinGW + Wine suite under that mode.
- Investigate and fix the current `test_transfer-xp` cleanup failure that
  appears under the forced `UNICODE` probe before making the macro flip the
  default contract.

## Verification Strategy

Required:

- `make -j8 -C crates/remote-exec-daemon-cpp BUILD_DIR=build/win-audit check-windows-xp`

Required before closing the hardening task:

- `make -j8 -C crates/remote-exec-daemon-cpp BUILD_DIR=build/win-audit-unicode WINDOWS_XP_PROD_CPPFLAGS='-I/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/include -I/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/third_party -DWIN32_LEAN_AND_MEAN -DWINVER=0x0501 -D_WIN32_WINNT=0x0501 -DUNICODE -D_UNICODE' WINDOWS_XP_TEST_CPPFLAGS='-I/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/include -I/home/chi/ddev/codex-remote-tools/remote-exec-mcp/crates/remote-exec-daemon-cpp/third_party -DWIN32_LEAN_AND_MEAN -DWINVER=0x0501 -D_WIN32_WINNT=0x0501 -DUNICODE -D_UNICODE' check-windows-xp`

Support checks as needed during implementation:

- `make -C crates/remote-exec-daemon-cpp BUILD_DIR=build/win-audit test-windows-xp-server-transport`
- `make -C crates/remote-exec-daemon-cpp BUILD_DIR=build/win-audit test-windows-xp-connection-manager`

## Assumptions And Open Questions

- The `UNICODE` probe failure in `test_transfer-xp` appears to be a real
  cleanup/runtime issue under Wine, not another missing-object problem. Confirm
  that before changing test policy or reducing scope.
- Logging environment reads such as `REMOTE_EXEC_LOG` and `RUST_LOG` are not
  part of this pass because their contents are ASCII control values, not host
  path or identity data.
- `GetProcAddress` symbol-name usage remains acceptable because those exports
  are ASCII API identifiers, not user text.
