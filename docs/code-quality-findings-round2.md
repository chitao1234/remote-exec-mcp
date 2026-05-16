# Code Quality Findings — Round 2

Audit date: 2026-05-17. Covers all production code in the workspace.

---

## Section 1: Error Handling Inconsistencies

### 1.1 `write_stdin` error wrapping loses error chain (moderate)
- **File**: `crates/remote-exec-broker/src/tools/exec.rs:140-141`
- `anyhow::anyhow!("write_stdin failed: {err}")` creates a new root error,
  discarding the original error chain. Other tool handlers return errors
  directly. Should use `.context("write_stdin failed")` to preserve the chain.

### 1.2 `into_http_rpc_parts` vs `into_rpc_parts` naming confusion (minor)
- **File**: `crates/remote-exec-host/src/error.rs:25-47`
- `into_http_rpc_parts` validates HTTP status range and normalizes to 500 on
  failure; `into_rpc_parts` does neither. The names don't convey this
  behavioral difference.

### 1.3 `PatchError` inconsistent with `define_domain_error!` macro (moderate)
- **File**: `crates/remote-exec-host/src/error.rs:181-258`
- `TransferError` and `ImageError` use `define_domain_error!` for consistent
  `Internal` variant handling and structured constructors. `PatchError` is
  manually implemented with a different structure and no `Internal` variant.

### 1.4 `PatchError::failed` vs `PatchError::failed_error` confusing names (minor)
- **File**: `crates/remote-exec-host/src/error.rs:199-204`
- One takes a message string, the other takes an error and calls `.to_string()`.
  Names like `failed_with_message` / `failed_from` would be clearer.

### 1.5 Missing path context in `open_transfer_import_body` (minor)
- **File**: `crates/remote-exec-broker/src/daemon_client/transfer.rs:206-218`
- IO errors from `File::open(archive_path)` don't include the file path.

### 1.6 String-based error format branching in `DaemonClientError::Display` (minor)
- **File**: `crates/remote-exec-broker/src/daemon_client/mod.rs:131`
- `message.starts_with("daemon returned malformed exec response: ")` to decide
  formatting is fragile.

---

## Section 2: Type Safety Gaps

### 2.1 `SessionStore::insert` takes four positional `String` arguments (minor)
- **File**: `crates/remote-exec-broker/src/session_store.rs:21-27`
- `insert(&self, target: String, daemon_session_id: String, daemon_instance_id: String, session_command: String)` — easy to accidentally swap arguments.

### 2.2 `SessionRecord` uses bare `String` for all ID fields (minor)
- **File**: `crates/remote-exec-broker/src/session_store.rs:7-14`
- `session_id`, `target`, `daemon_session_id`, `daemon_instance_id`, and
  `session_command` are all `String`. At minimum `session_id` and
  `daemon_session_id` could be newtypes.

### 2.3 Raw `u16` for HTTP status codes in `HostRpcError` (moderate)
- **File**: `crates/remote-exec-host/src/error.rs:8`
- Requires runtime validation. `http::StatusCode` or a newtype that enforces
  validity at construction would eliminate the check.

### 2.4 `ForwardLimits` uses mixed `u64`/`usize` with silent truncation (moderate)
- **File**: `crates/remote-exec-broker/src/port_forward/supervisor.rs:87-94, 111-121`
- `From<ForwardPortLimitSummary>` casts `u64` to `usize` via `as usize`, which
  silently truncates on 32-bit platforms.

### 2.5 `TransferFilesInput` allows mutually exclusive invalid states (moderate)
- **File**: `crates/remote-exec-proto/src/public/transfer.rs:26-51`
- Has both `source: Option<TransferEndpoint>` and `sources: Vec<TransferEndpoint>`.
  Mutual exclusivity enforced only at runtime. An enum would make invalid states
  unrepresentable.

### 2.6 `ForwardPortEntry::new_open` takes 7 positional parameters (minor)
- **File**: `crates/remote-exec-proto/src/public/forward_ports.rs:188-229`
- Multiple `String` values that could be transposed.

### 2.7 `Timestamp` wraps a `String` of seconds (minor)
- **File**: `crates/remote-exec-proto/src/public/forward_ports.rs:109-127`
- Does not encode units. A `u64` or `SystemTime` would be more self-documenting.

---

## Section 3: Structural Issues

### 3.1 `TargetConfig::transport_kind()` has hidden validation side-effect (moderate)
- **File**: `crates/remote-exec-broker/src/config.rs:202-209`
- Internally calls `self.validate(name)?`. A caller reading `transport_kind`
  expects a simple getter, not a full validation pass.

### 3.2 Parallel tool dispatch in `client.rs` must stay in sync (moderate)
- **File**: `crates/remote-exec-broker/src/tools/exec.rs:170-212`
- `call_direct_tool` builds a second dispatch table via `invoke_tool!` macro
  that mirrors the `#[tool_router]` in `mcp_server.rs`. No test verifies they
  stay in sync (unlike `tool_router_matches_registry_names` for the MCP side).

### 3.3 Duplicated `From<DaemonConfig>` for owned and borrowed (moderate)
- **File**: `crates/remote-exec-daemon/src/config/mod.rs:181-222`
- `From<DaemonConfig>` (owned) and `From<&DaemonConfig>` (borrowed) do
  identical field-by-field mapping; the reference version clones. Adding a field
  requires updating both in lockstep.

### 3.4 `TransferImportMetadata` is a bare type alias (minor)
- **File**: `crates/remote-exec-proto/src/transfer.rs:134`
- `pub type TransferImportMetadata = TransferImportRequest;` — conflates
  "metadata parsed from HTTP headers" with "a full import request struct".

---

## Section 4: Dead Code / Unnecessary Complexity

### 4.1 `#[allow(clippy::too_many_arguments)]` on `open_tunnel_with_role` (moderate)
- **File**: `crates/remote-exec-broker/src/port_forward/supervisor/tunnel_open.rs:121-131`
- 8 parameters. The `TunnelOpenContext` is always created inline at call sites.
  Combining non-context params into a struct would remove the suppression.

### 4.2 Deprecated `source` field in `TransferFilesResult` (minor)
- **File**: `crates/remote-exec-broker/src/tools/transfer/format.rs:29`
- `source: (sources.len() == 1).then(|| sources[0].clone())` alongside the
  always-present `sources` vec. If no consumers need it, remove it.

### 4.3 `ForwardSide` enum exists only for two format strings (minor)
- **File**: `crates/remote-exec-broker/src/port_forward/supervisor/open.rs:38-42, 345-360`
- Used in only two places. Could be inlined.

### 4.4 `let _ = (cmd, cwd);` after variables are already consumed (minor)
- **File**: `crates/remote-exec-host/src/exec/session/spawn.rs:149`
- Dead suppression of unused-variable warnings for variables that were already
  used.

### 4.5 `open_v4_resumable_tcp_listener` test helper is a trivial passthrough (minor)
- **File**: `crates/remote-exec-host/src/port_forward/tests/mod.rs:1138-1140`
- Vestige of v3→v4 protocol migration.

### 4.6 `ExecRequestFailure`, `TransferFailure`, `ImageFailure` store redundant message (C++) (minor)
- **File**: `crates/remote-exec-daemon-cpp/include/rpc_failures.h:29-43`
- Inherit from `std::runtime_error` (stores message via `what()`) AND keep a
  public `std::string message` member — two copies of every message.

---

## Section 5: C++ Duplicated Logic

### 5.1 `is_http_token_char` duplicated (minor)
- `crates/remote-exec-daemon-cpp/src/http_codec.cpp:44-69`
- `crates/remote-exec-daemon-cpp/src/http_request.cpp:14-39`

### 5.2 `lowercase_ascii` duplicated across four files (minor)
- `src/text_utils.cpp:17-22` (canonical public version)
- `src/platform.cpp:23-28`, `src/shell_policy.cpp:25-30`,
  `src/path_policy.cpp:13-18` (anonymous namespace copies)

### 5.3 `wide_from_utf8` duplicated across three files (Win32) (minor)
- `src/path_utils.cpp:59-77` (public version)
- `src/path_compare.cpp:75-93`, `src/process_session_win32.cpp:20-37` (copies)

### 5.4 `is_separator` duplicated (minor)
- `src/filesystem_sandbox.cpp:55-60`
- `src/path_compare.cpp:24-29`

### 5.5 `is_ascii_alpha` duplicated (minor)
- `src/path_compare.cpp:20-22`
- `src/path_policy.cpp:9-11`

### 5.6 `transfer_warnings_json` duplicated (minor)
- `src/transfer_ops_tar.cpp:158-167`
- `src/transfer_http_codec.cpp:115-124`

---

## Section 6: C++ Long Functions

### 6.1 `parse_patch` — 97 lines (moderate)
- **File**: `crates/remote-exec-daemon-cpp/src/patch_engine.cpp:359-455`
- Multi-branch parsing state machine. Each action-type parser (Add, Delete,
  Update) could be extracted.

### 6.2 `tunnel_open` — 89 lines (moderate)
- **File**: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp:247-335`
- Handles session creation, resumption, and connect mode in one function.

### 6.3 `import_directory_from_tar` — 91 lines (moderate)
- **File**: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp:406-496`
- Tar entry processing loop with branches for each entry type.

---

## Section 7: C++ Consistency

### 7.1 Remaining `static const` in headers should be `constexpr` (minor)
- `include/port_tunnel_frame.h:9-11`
- `include/transfer_ops.h:58-59`
- `include/session_store.h:13`
- `config.h` already uses `constexpr`. These should match.

---

## Section 8: Security-Adjacent

### 8.1 Unix private key file briefly has default permissions after rename (moderate)
- **File**: `crates/remote-exec-pki/src/write/unix.rs:5-19`
- Writes temp file, renames to final path, then sets permissions. Between
  rename and `set_permissions`, the file is world-readable. Should set
  permissions on the temp file before renaming.

### 8.2 Non-unique temporary file path in PKI write (minor)
- **File**: `crates/remote-exec-pki/src/write.rs:122-127`
- `temporary_path` always generates `{filename}.tmp`. Concurrent invocations
  could overwrite each other's temp files.

---

## Summary

| Severity    | Count |
|-------------|-------|
| Significant | 0     |
| Moderate    | 13    |
| Minor       | 27    |
| Total       | 40    |

### Top priorities (moderate findings worth addressing):
1. **1.1** — `write_stdin` error chain loss (easy fix)
2. **2.4** — `ForwardLimits` u64→usize truncation (easy fix)
3. **2.5** — `TransferFilesInput` invalid states (design improvement)
4. **3.1** — `transport_kind()` hidden validation (easy refactor)
5. **3.3** — Duplicated `From<DaemonConfig>` impls (easy DRY)
6. **4.1** — `open_tunnel_with_role` too many args (easy refactor)
7. **8.1** — PKI private key permission window (security fix)
