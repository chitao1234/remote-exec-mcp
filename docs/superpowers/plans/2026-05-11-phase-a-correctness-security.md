# Phase A Correctness Security Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve only Phase A correctness and security items from `docs/CODE_AUDIT_ROUND2.md`, with validation and focused tests for each risk area.

**Architecture:** Treat the audit as review input, not as the live contract. Fix the active bugs in small, reviewable commits: exec output decoding, transfer archive bounds/streaming, C++ symlink and size validation, port-forward lock/state semantics, queue-control delivery, C++ tunnel teardown, and PKI documentation/platform follow-up. Do not implement transactional `apply_patch`; document that multi-file patch application is intentionally non-transactional.

**Tech Stack:** Rust 2024 workspace with Tokio, Axum/Hyper, Serde, tar/zstd, rcgen PKI; C++11 daemon with POSIX and Windows XP-compatible builds; existing Cargo integration tests and C++ make test targets.

---

## Scope

Included audit items: `#1` through `#12` from `docs/CODE_AUDIT_ROUND2.md`.

Explicitly excluded from this plan: Phase B and Phase C audit items `#13+`.

Special handling: item `#5` is intentional product behavior. The task is to document the non-transactional behavior clearly, not to make `apply_patch` transactional.

## Current Validation Snapshot

- `#1` is present in `crates/remote-exec-host/src/exec/session/spawn.rs`: `String::from_utf8_lossy` is called per pipe read.
- `#2` is present in `crates/remote-exec-host/src/transfer/archive/import.rs`: file entries are read into a `Vec` before writing.
- `#3` is present in C++ import helpers: `uint64_t` archive sizes are cast to `std::size_t` for string allocation/substr without a reusable range guard.
- `#4` is present in `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`: preserved symlink targets are passed to `symlink()` without target validation.
- `#5` is intentionally non-transactional per audit note.
- `#6` is present in `crates/remote-exec-broker/src/port_forward/supervisor.rs`: listen reconnect/close paths can hold `ListenSessionControl.state` across awaited tunnel operations.
- `#7` is present in `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`: `run()` has no `catch (...)`.
- `#8` is present in Rust host TCP EOF handling and broker tunnel heartbeat ack handling: control messages can be dropped by `try_send`.
- `#9` is present in `crates/remote-exec-host/src/port_forward/tunnel.rs`: open mode is checked and later set under separate locks.
- `#10` is present in `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`: stream cleanup returns before releasing all active stream budget.
- `#11` is present in `crates/remote-exec-host/src/port_forward/session.rs`: expiry tasks are detached and not cancelled on reattach.
- `#12` remains a Windows-specific PKI limitation: Unix modes are enforced, Windows ACL hardening is not implemented.

## File Structure

- `crates/remote-exec-host/src/exec/session/spawn.rs`: add streaming UTF-8 decoder helper and unit tests.
- `crates/remote-exec-proto/src/transfer.rs`: add transfer limit data types and defaults shared by Rust broker/daemon/host.
- `crates/remote-exec-host/src/config/mod.rs`: carry `TransferLimits` through `HostRuntimeConfig` and `EmbeddedHostConfig`.
- `crates/remote-exec-daemon/src/config/mod.rs`: deserialize Rust daemon transfer limits and pass them into host runtime.
- `crates/remote-exec-broker/src/config.rs`, `crates/remote-exec-broker/src/local_backend.rs`, `crates/remote-exec-broker/src/local_port_backend.rs`: pass transfer limits for embedded local host runtimes.
- `crates/remote-exec-host/src/transfer/archive/import.rs`: enforce transfer limits and stream file entries to disk.
- `crates/remote-exec-daemon/tests/transfer_rpc.rs`: add integration coverage for archive limit enforcement.
- `crates/remote-exec-daemon-cpp/include/config.h`, `src/config.cpp`, `README.md`, `tests/test_config.cpp`: add C++ transfer limit configuration.
- `crates/remote-exec-daemon-cpp/include/transfer_ops.h`, `src/transfer_ops_import.cpp`, `src/transfer_ops_tar.cpp`, `src/transfer_ops_internal.h`, `src/transfer_ops_fs.cpp`, `src/server_request_utils.cpp`, `src/server_route_transfer.cpp`, `tests/test_transfer.cpp`: enforce C++ size and symlink target rules.
- `crates/remote-exec-broker/src/port_forward/supervisor.rs`: release listen session locks before network I/O.
- `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`: add catch-all teardown path.
- `crates/remote-exec-host/src/port_forward/tcp.rs`, `crates/remote-exec-broker/src/port_forward/tunnel.rs`: make control delivery reliable under queue pressure.
- `crates/remote-exec-host/src/port_forward/tunnel.rs`: make tunnel open-mode transition atomic under one guard.
- `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`: release all active TCP stream budgets even when close frames fail.
- `crates/remote-exec-host/src/port_forward/session.rs`, `types.rs` if needed: store and cancel session expiry handles/tokens on reattach.
- `README.md`, `skills/using-remote-exec-mcp/SKILL.md`, `configs/*.example.toml`, `crates/remote-exec-daemon-cpp/README.md`: document transfer limits, non-transactional patch semantics, and Windows private-key ACL limitation.

---

### Task 1: Save The Phase A Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-a-correctness-security.md`
- Test/Verify: `git status --short docs/superpowers/plans/2026-05-11-phase-a-correctness-security.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked plan artifact only.

- [ ] **Step 1: Verify this plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-a-correctness-security.md`
Expected: command exits successfully.

- [ ] **Step 2: Review the plan heading and scope.**

Run: `sed -n '1,80p' docs/superpowers/plans/2026-05-11-phase-a-correctness-security.md`
Expected: output names Phase A only, includes the required agentic-worker header, and explicitly excludes items `#13+`.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-a-correctness-security.md
git commit -m "docs: plan phase a audit fixes"
```

### Task 2: Preserve UTF-8 Across Pipe Read Boundaries

**Finding:** `#1`

**Files:**
- Modify: `crates/remote-exec-host/src/exec/session/spawn.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host exec_pipe_decoder`
  - `cargo test -p remote-exec-broker --test mcp_exec`

**Testing approach:** TDD
Reason: The bug is a deterministic pure decoding behavior. Unit tests can force split multibyte sequences without spawning a real process.

- [ ] **Step 1: Add failing decoder tests in `spawn.rs`.**

Append this test module to `crates/remote-exec-host/src/exec/session/spawn.rs` if no test module exists there:

```rust
#[cfg(test)]
mod exec_pipe_decoder_tests {
    use super::Utf8PipeDecoder;

    #[test]
    fn split_multibyte_codepoint_is_emitted_once() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[0xe4, 0xbd]), None);
        assert_eq!(decoder.push(&[0xa0]), Some("你".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn invalid_complete_sequence_is_lossy_but_trailing_prefix_is_preserved() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[0xff, b'a', 0xf0, 0x9f]), Some("\u{fffd}a".to_string()));
        assert_eq!(decoder.push(&[0x98, 0x80]), Some("😀".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn unfinished_sequence_is_replaced_on_finish() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[b'a', 0xe4, 0xbd]), Some("a".to_string()));
        assert_eq!(decoder.finish(), Some("\u{fffd}".to_string()));
    }
}
```

- [ ] **Step 2: Run the focused failing test.**

Run: `cargo test -p remote-exec-host exec_pipe_decoder`
Expected: compile fails because `Utf8PipeDecoder` does not exist.

- [ ] **Step 3: Implement `Utf8PipeDecoder` and use it in `spawn_output_reader`.**

Add this helper above `spawn_output_reader`:

```rust
struct Utf8PipeDecoder {
    pending: Vec<u8>,
}

impl Utf8PipeDecoder {
    fn new() -> Self {
        Self { pending: Vec::new() }
    }

    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(valid) => {
                if valid.is_empty() {
                    None
                } else {
                    let output = valid.to_string();
                    self.pending.clear();
                    Some(output)
                }
            }
            Err(err) => {
                let valid_up_to = err.valid_up_to();
                if valid_up_to == 0 {
                    if err.error_len().is_none() {
                        return None;
                    }
                    let output = String::from_utf8_lossy(&self.pending).into_owned();
                    self.pending.clear();
                    return Some(output);
                }

                let output = String::from_utf8_lossy(&self.pending[..valid_up_to]).into_owned();
                self.pending.drain(..valid_up_to);
                Some(output)
            }
        }
    }

    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            None
        } else {
            let output = String::from_utf8_lossy(&self.pending).into_owned();
            self.pending.clear();
            Some(output)
        }
    }
}
```

Change `spawn_output_reader` to instantiate the decoder before the loop, call `decoder.push(&buffer[..read])` on each successful read, and send only when `Some(chunk)` is returned. After `Ok(0)`, send `decoder.finish()` if present before breaking. On `Err(_)`, send `decoder.finish()` if present before breaking.

- [ ] **Step 4: Run focused and broker exec tests.**

Run: `cargo test -p remote-exec-host exec_pipe_decoder`
Expected: all decoder tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_exec`
Expected: broker exec tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-host/src/exec/session/spawn.rs
git commit -m "fix: preserve utf8 pipe output boundaries"
```

### Task 3: Add Rust Transfer Limits To Config And Protocol Types

**Finding:** `#2`

**Files:**
- Modify: `crates/remote-exec-proto/src/transfer.rs`
- Modify: `crates/remote-exec-host/src/config/mod.rs`
- Modify: `crates/remote-exec-host/src/state.rs`
- Modify: `crates/remote-exec-daemon/src/config/mod.rs`
- Modify: `crates/remote-exec-broker/src/config.rs`
- Modify: `crates/remote-exec-broker/src/local_backend.rs`
- Modify: `crates/remote-exec-broker/src/local_port_backend.rs`
- Modify: `configs/daemon.example.toml`
- Modify: `configs/broker.example.toml`
- Test/Verify:
  - `cargo test -p remote-exec-proto`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`
  - `cargo test -p remote-exec-broker --test mcp_transfer`

**Testing approach:** existing tests + config unit coverage
Reason: This task adds plumbed configuration without enforcing behavior yet. Existing compilation and config construction paths catch missed fields.

- [ ] **Step 1: Add shared transfer limit types.**

Add to `crates/remote-exec-proto/src/transfer.rs`:

```rust
pub const DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_TRANSFER_MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct TransferLimits {
    pub max_archive_bytes: u64,
    pub max_entry_bytes: u64,
}

impl Default for TransferLimits {
    fn default() -> Self {
        Self {
            max_archive_bytes: DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES,
            max_entry_bytes: DEFAULT_TRANSFER_MAX_ENTRY_BYTES,
        }
    }
}

impl TransferLimits {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.max_archive_bytes > 0,
            "transfer_limits.max_archive_bytes must be greater than zero"
        );
        anyhow::ensure!(
            self.max_entry_bytes > 0,
            "transfer_limits.max_entry_bytes must be greater than zero"
        );
        anyhow::ensure!(
            self.max_entry_bytes <= self.max_archive_bytes,
            "transfer_limits.max_entry_bytes must be less than or equal to transfer_limits.max_archive_bytes"
        );
        Ok(())
    }
}
```

If `remote-exec-proto` does not currently depend on `anyhow`, add `anyhow = { workspace = true }` to `crates/remote-exec-proto/Cargo.toml`; otherwise use the existing dependency.

- [ ] **Step 2: Add limits to host config.**

In `crates/remote-exec-host/src/config/mod.rs`, import `remote_exec_proto::transfer::TransferLimits` and add `pub transfer_limits: TransferLimits` to both `HostRuntimeConfig` and `EmbeddedHostConfig`. In `EmbeddedHostConfig::into_host_runtime_config`, pass `self.transfer_limits`.

In `crates/remote-exec-host/src/state.rs`, call `config.transfer_limits.validate()?` beside existing config validation.

- [ ] **Step 3: Add limits to daemon config.**

In `crates/remote-exec-daemon/src/config/mod.rs`, import `TransferLimits`, add this field to `DaemonConfig`:

```rust
#[serde(default)]
pub transfer_limits: TransferLimits,
```

Thread it through `EmbeddedDaemonConfig::into_daemon_config`, `From<DaemonConfig> for HostRuntimeConfig`, `From<EmbeddedHostConfig> for DaemonConfig`, and test helpers that construct `DaemonConfig`.

- [ ] **Step 4: Add limits to broker local config.**

In `crates/remote-exec-broker/src/config.rs`, import `TransferLimits` and add:

```rust
#[serde(default)]
pub transfer_limits: TransferLimits,
```

to `BrokerConfig` for broker-host `transfer_files target="local"` and to `LocalTargetConfig` for embedded local host runtime. Pass the correct values in `LocalTargetConfig::embedded_host_config`, `local_backend.rs`, and `local_port_backend.rs`. `local_port_backend.rs` can use `TransferLimits::default()` because it does not process transfer archives.

- [ ] **Step 5: Document config keys.**

In `configs/daemon.example.toml`, add:

```toml
#[transfer_limits]
#max_archive_bytes = 536870912
#max_entry_bytes = 536870912
```

In `configs/broker.example.toml`, add the same table near broker-host transfer settings and mention that it applies to broker-host local transfer import/export processing.

- [ ] **Step 6: Run focused verification.**

Run: `cargo test -p remote-exec-proto`
Expected: proto tests pass.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: daemon transfer tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: broker transfer tests pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-proto crates/remote-exec-host crates/remote-exec-daemon crates/remote-exec-broker configs/daemon.example.toml configs/broker.example.toml
git commit -m "feat: add rust transfer size limits"
```

### Task 4: Stream Rust Archive Entries And Enforce Transfer Limits

**Finding:** `#2`

**Files:**
- Modify: `crates/remote-exec-host/src/transfer/archive/import.rs`
- Modify: `crates/remote-exec-host/src/transfer/mod.rs`
- Modify: `crates/remote-exec-daemon/tests/transfer_rpc.rs`
- Modify: `tests/support/transfer_archive.rs` if a helper is useful
- Test/Verify:
  - `cargo test -p remote-exec-daemon --test transfer_rpc transfer_import_rejects_entry_over_limit`
  - `cargo test -p remote-exec-daemon --test transfer_rpc`
  - `cargo test -p remote-exec-broker --test mcp_transfer`

**Testing approach:** TDD
Reason: The limit must reject malicious archive metadata before allocating a full entry body, and existing transfer behavior must remain unchanged.

- [ ] **Step 1: Add a failing daemon integration test.**

Add to `crates/remote-exec-daemon/tests/transfer_rpc.rs`:

```rust
#[tokio::test]
async fn transfer_import_rejects_entry_over_limit() {
    let fixture = support::spawn::spawn_daemon_with_config("builder-a", |config| {
        config.transfer_limits.max_archive_bytes = 4096;
        config.transfer_limits.max_entry_bytes = 8;
    })
    .await;
    let destination = fixture.workdir.join("too-large.txt");
    let body = support::transfer_archive::raw_tar_file_with_path(".remote-exec-file", b"0123456789");

    let response = fixture
        .raw_post_bytes(
            "/v1/transfer/import",
            &import_headers(destination.display(), "replace", "true", "file"),
            body,
        )
        .await;

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "transfer_failed");
    assert!(err.message.contains("exceeds transfer entry limit"));
    assert!(!destination.exists());
}
```

If `spawn_daemon_with_config` does not exist, add it to `crates/remote-exec-daemon/tests/support/spawn.rs` by factoring the existing spawn helper so tests can mutate `DaemonConfig` before launching.

- [ ] **Step 2: Run the failing test.**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc transfer_import_rejects_entry_over_limit`
Expected: compile fails if `spawn_daemon_with_config` is missing, or the test fails because the oversized entry is accepted.

- [ ] **Step 3: Thread limits into archive import.**

Change these functions to accept `TransferLimits`:

```rust
import_archive_from_file(..., limits: TransferLimits)
import_archive_from_async_reader(..., limits: TransferLimits)
extract_archive(..., limits: TransferLimits)
extract_archive_from_reader(..., limits: TransferLimits)
extract_single_file_archive(..., limits: TransferLimits)
extract_tree_archive(..., limits: TransferLimits)
extract_tree_archive_entry(..., limits: TransferLimits)
write_archive_file(..., limits: TransferLimits)
```

In `crates/remote-exec-host/src/transfer/mod.rs`, call archive import with `state.config.transfer_limits`.

- [ ] **Step 4: Replace read-to-end with bounded streaming copy.**

In `write_archive_file`, replace the `Vec` allocation with a direct file copy. The function shape should be:

```rust
fn write_archive_file<R: Read>(
    entry: &mut tar::Entry<R>,
    path: &Path,
    limits: TransferLimits,
    copied_so_far: u64,
) -> anyhow::Result<u64> {
    ensure_not_existing_symlink(path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let entry_size = entry.header().size()?;
    ensure_entry_within_limits(entry_size, copied_so_far, limits)?;

    let mut output = std::fs::File::create(path)?;
    let copied = std::io::copy(&mut entry.take(entry_size), &mut output)?;
    if copied != entry_size {
        anyhow::bail!("truncated archive entry");
    }
    restore_executable_bits(path, entry.header().mode()?)?;
    Ok(copied)
}
```

Add:

```rust
fn ensure_entry_within_limits(
    entry_size: u64,
    copied_so_far: u64,
    limits: TransferLimits,
) -> anyhow::Result<()> {
    if entry_size > limits.max_entry_bytes {
        anyhow::bail!(
            "archive entry size {entry_size} exceeds transfer entry limit {}",
            limits.max_entry_bytes
        );
    }
    if copied_so_far.saturating_add(entry_size) > limits.max_archive_bytes {
        anyhow::bail!(
            "archive byte count exceeds transfer archive limit {}",
            limits.max_archive_bytes
        );
    }
    Ok(())
}
```

For single-file import, pass `0` as `copied_so_far`. For directory/multiple imports, pass `summary.bytes_copied` before adding the returned count.

- [ ] **Step 5: Run focused and regression tests.**

Run: `cargo test -p remote-exec-daemon --test transfer_rpc transfer_import_rejects_entry_over_limit`
Expected: new test passes.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: all daemon transfer tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: broker transfer tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src/transfer crates/remote-exec-daemon/tests tests/support
git commit -m "fix: stream and bound rust transfer imports"
```

### Task 5: Harden C++ Archive Size Handling

**Finding:** `#3`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/config.h`
- Modify: `crates/remote-exec-daemon-cpp/src/config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/transfer_ops.h`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_tar.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/server_route_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_config.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-transfer`
  - `make -C crates/remote-exec-daemon-cpp check-posix`
  - `make -C crates/remote-exec-daemon-cpp check-windows-xp`

**Testing approach:** TDD plus cross-build verification
Reason: The bug is most dangerous on 32-bit XP builds, so compile coverage for that target matters even if the test runs on the host.

- [ ] **Step 1: Add C++ transfer limit config.**

In `include/config.h`, add:

```cpp
struct TransferLimitConfig {
    std::uint64_t max_archive_bytes;
    std::uint64_t max_entry_bytes;
};
```

Add `TransferLimitConfig transfer_limits;` to `DaemonConfig`.

Add defaults:

```cpp
static const std::uint64_t DEFAULT_TRANSFER_MAX_ARCHIVE_BYTES = 512ULL * 1024ULL * 1024ULL;
static const std::uint64_t DEFAULT_TRANSFER_MAX_ENTRY_BYTES = 512ULL * 1024ULL * 1024ULL;
```

In `src/config.cpp`, read optional keys `transfer_max_archive_bytes` and `transfer_max_entry_bytes`, defaulting to those constants, and validate both are non-zero and `max_entry_bytes <= max_archive_bytes`. Update `tests/test_config.cpp` to assert explicit and default values.

- [ ] **Step 2: Add failing C++ transfer tests.**

In `tests/test_transfer.cpp`, add a helper that constructs a tar header with a declared size larger than the actual body:

```cpp
static std::string tar_with_declared_file_size(
    const std::string& path,
    std::uint64_t declared_size
) {
    std::string header(512, '\0');
    set_bytes(&header, 0, 100, path);
    header.replace(100, 8, octal_field(8, 0644));
    header.replace(124, 12, octal_field(12, declared_size));
    header[156] = '0';
    set_bytes(&header, 257, 6, "ustar ");
    set_bytes(&header, 263, 2, " \0");
    write_checksum(&header);

    std::string archive;
    archive.append(header);
    archive.append(1024, '\0');
    return archive;
}
```

Add:

```cpp
static void assert_transfer_rejects_entry_size_over_limit() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-size-limit";
    fs::remove_all(root);
    fs::create_directories(root);

    TransferLimitConfig limits;
    limits.max_archive_bytes = 4096U;
    limits.max_entry_bytes = 8U;

    bool rejected = false;
    try {
        (void)import_path(
            tar_with_single_file(SINGLE_FILE_ENTRY, "0123456789"),
            TransferSourceType::File,
            (root / "dest.txt").string(),
            "replace",
            true,
            TransferSymlinkMode::Preserve,
            limits
        );
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("transfer entry limit") != std::string::npos;
    }
    assert(rejected);
}

static void assert_transfer_rejects_unrepresentable_tar_size() {
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-huge-size";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(
            tar_with_declared_file_size(SINGLE_FILE_ENTRY, 077777777777ULL),
            TransferSourceType::File,
            (root / "dest.txt").string(),
            "replace",
            true
        );
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("too large") != std::string::npos ||
                   failure.message.find("limit") != std::string::npos;
    }
    assert(rejected);
}
```

Call both from `main()`.

- [ ] **Step 3: Run failing C++ transfer test.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: compile fails because `TransferLimitConfig` is not wired into `import_path`, or tests fail because oversized entries are accepted.

- [ ] **Step 4: Add reusable size guards.**

In `src/transfer_ops_internal.h`, declare:

```cpp
void ensure_u64_fits_size_t(std::uint64_t value, const std::string& label);
void ensure_transfer_entry_within_limits(
    std::uint64_t entry_size,
    std::uint64_t copied_so_far,
    const TransferLimitConfig& limits
);
```

In `src/transfer_ops_import.cpp`, implement both using `std::numeric_limits<std::size_t>::max()` and the configured limits. Use `ensure_u64_fits_size_t` before every `static_cast<std::size_t>(size)` used for allocation, substr, or padded-length arithmetic, including `read_exact_string` and the `transfer_ops_tar.cpp` GNU long-name path.

- [ ] **Step 5: Thread limits through import APIs.**

Extend `import_path` and `import_path_from_reader` in `include/transfer_ops.h` with a trailing defaulted parameter:

```cpp
const TransferLimitConfig& limits = default_transfer_limit_config()
```

If C++ does not permit binding a default reference to a function return cleanly in this code style, add overloads: existing signatures call new signatures with `default_transfer_limit_config()`.

Thread `limits` into `import_file_from_tar`, `import_directory_from_tar`, `copy_reader_to_file`, `read_gnu_long_name_from_reader`, and transfer summary reads. Enforce `max_entry_bytes` before copying or allocating any entry body, and enforce `max_archive_bytes` using the summary byte counter for copied file data plus oversized metadata entries that are fully read as strings.

In `server_request_utils.cpp`, store `state.config.transfer_limits` in `TransferImportRequestSpec`. In `server_route_transfer.cpp`, pass it to `import_path`.

- [ ] **Step 6: Document C++ transfer limits.**

In `crates/remote-exec-daemon-cpp/README.md`, add:

```toml
# transfer_max_archive_bytes = 536870912
# transfer_max_entry_bytes = 536870912
```

and mention that these limits bound imported archive file entries and in-memory tar metadata bodies.

- [ ] **Step 7: Run C++ verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: transfer tests pass.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: POSIX build and tests pass.

Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
Expected: XP cross build/test target succeeds.

- [ ] **Step 8: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/tests crates/remote-exec-daemon-cpp/README.md
git commit -m "fix: bound cpp transfer archive sizes"
```

### Task 6: Validate C++ Symlink Import Targets

**Finding:** `#4`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_fs.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/transfer_ops_internal.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_request_utils.cpp`
- Modify: `crates/remote-exec-daemon-cpp/include/server_request_utils.h`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_routes.cpp` if sandbox-level coverage is added there
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-transfer`
  - `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** TDD
Reason: Crafted archive symlink targets are attacker-controlled input and need explicit rejection tests for absolute paths, parent traversal, and sandbox escape.

- [ ] **Step 1: Add failing symlink target tests.**

In `tests/test_transfer.cpp`, add POSIX-only tests:

```cpp
#ifndef _WIN32
static void assert_symlink_import_rejects_absolute_target() {
    std::string archive;
    append_tar_symlink(&archive, "bad-link", "/etc/passwd");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-absolute";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(
            archive,
            TransferSourceType::Directory,
            (root / "dest").string(),
            "replace",
            true
        );
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("symlink target") != std::string::npos;
    }
    assert(rejected);
}

static void assert_symlink_import_rejects_parent_target() {
    std::string archive;
    append_tar_symlink(&archive, "bad-link", "../escape.txt");
    finalize_tar(archive);

    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-symlink-parent";
    fs::remove_all(root);
    fs::create_directories(root);

    bool rejected = false;
    try {
        (void)import_path(
            archive,
            TransferSourceType::Directory,
            (root / "dest").string(),
            "replace",
            true
        );
    } catch (const TransferFailure& failure) {
        rejected = failure.message.find("symlink target") != std::string::npos;
    }
    assert(rejected);
}
#endif
```

Call them from `main()` under `#ifndef _WIN32`.

- [ ] **Step 2: Run failing transfer test.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: new tests fail because absolute and parent-traversal symlink targets are accepted.

- [ ] **Step 3: Add symlink target validation helper.**

In `src/transfer_ops_import.cpp`, add:

```cpp
std::string validate_relative_symlink_target(const std::string& raw_target) {
    const std::string normalized = normalize_archive_separators(raw_target);
    if (normalized.empty() || normalized[0] == '/' || normalized.rfind("//", 0) == 0) {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink target must be relative");
    }
    if (normalized.size() >= 2 &&
        std::isalpha(static_cast<unsigned char>(normalized[0])) != 0 &&
        normalized[1] == ':') {
        throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink target must be relative");
    }
    const std::vector<std::string> parts = split_archive_path(normalized);
    for (std::size_t i = 0; i < parts.size(); ++i) {
        if (parts[i].empty() || parts[i] == "." || parts[i] == "..") {
            throw TransferFailure(TransferRpcCode::SourceUnsupported, "archive symlink target escapes destination");
        }
    }
    return normalized;
}
```

Use this validated target before calling `write_symlink` in both single-file and directory import paths.

- [ ] **Step 4: Apply sandbox authorization to resolved symlink targets.**

Extend `TransferImportRequestSpec` with an optional authorizer:

```cpp
typedef void (*TransferPathAuthorizer)(const std::string& path);
```

or use the existing C++ project callback style if one is already used for patch authorization. The authorizer should authorize `SANDBOX_WRITE`.

When preserving a symlink, compute the lexical resolved target path by joining the symlink parent directory with the validated relative target using `path_utils::parent_directory` and `join_path`, then call the authorizer before `write_symlink`. This check is lexical; do not follow the target, because the target may not exist yet.

- [ ] **Step 5: Run C++ transfer and POSIX checks.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: new symlink tests pass.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: POSIX build and tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/tests
git commit -m "fix: validate cpp transfer symlink targets"
```

### Task 7: Document Non-Transactional Patch Semantics

**Finding:** `#5`

**Files:**
- Modify: `README.md`
- Modify: `skills/using-remote-exec-mcp/SKILL.md`
- Modify: `crates/remote-exec-daemon-cpp/README.md`
- Test/Verify:
  - `rg -n "non-transactional|not transactional|partial" README.md skills/using-remote-exec-mcp/SKILL.md crates/remote-exec-daemon-cpp/README.md`

**Testing approach:** documentation verification
Reason: The intended behavior is documentation, not behavior change.

- [ ] **Step 1: Update README patch contract.**

In `README.md`, near the existing `apply_patch` behavior bullets, add:

```markdown
- `apply_patch` is intentionally non-transactional across multiple file actions. Actions are applied in order; if a later action fails, earlier successful file changes are left on disk and the returned error describes the failure point. Callers that need all-or-nothing behavior should split work into smaller patches or verify state before and after applying.
```

- [ ] **Step 2: Update MCP skill patch guidance.**

In `skills/using-remote-exec-mcp/SKILL.md`, in the `apply_patch` section, add:

```markdown
- `apply_patch` is not transactional across multiple file actions. If one patch contains several file edits and a later edit fails, earlier successful edits remain applied. Prefer smaller patches when partial application would be hard to recover from.
```

- [ ] **Step 3: Update C++ daemon README.**

In `crates/remote-exec-daemon-cpp/README.md`, add the same C++-specific note in the patch support section:

```markdown
- `apply_patch` follows the project-wide non-transactional patch contract: multi-file patches apply actions in order, and earlier successful edits remain if a later action fails.
```

- [ ] **Step 4: Verify docs contain the contract.**

Run: `rg -n "non-transactional|not transactional" README.md skills/using-remote-exec-mcp/SKILL.md crates/remote-exec-daemon-cpp/README.md`
Expected: all three files contain the non-transactional wording.

- [ ] **Step 5: Commit.**

```bash
git add README.md skills/using-remote-exec-mcp/SKILL.md crates/remote-exec-daemon-cpp/README.md
git commit -m "docs: document non transactional patch behavior"
```

### Task 8: Release Broker Listen-Session Locks Before Network I/O

**Finding:** `#6`

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/supervisor.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs` if an integration regression seam is practical
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`

**Testing approach:** existing tests + targeted code review
Reason: The bug is lock scope around live network I/O. Existing reconnect tests cover behavior; the key verification is that no awaited reconnect/open/close operation remains under `control.state.lock()`.

- [ ] **Step 1: Capture current lock sites.**

Run: `rg -n "control\\.state\\.lock|resume_listen_session_inner|close_listener_on_tunnel|close_tunnel_generation" crates/remote-exec-broker/src/port_forward/supervisor.rs`
Expected: identify lock sites in `try_resume_listen_tunnel`, `close_listen_session`, and helper paths.

- [ ] **Step 2: Refactor `try_resume_listen_tunnel`.**

Change it to:

```rust
async fn try_resume_listen_tunnel(
    control: &Arc<ListenSessionControl>,
) -> anyhow::Result<Arc<PortTunnel>> {
    let tunnel = resume_listen_session_inner(control).await?;
    let mut state = control.state.lock().await;
    state.current_tunnel = Some(tunnel.clone());
    Ok(tunnel)
}
```

- [ ] **Step 3: Refactor `close_listen_session`.**

Read the current tunnel under the lock, then drop the lock before awaiting network operations:

```rust
let current_tunnel = {
    let state = control.state.lock().await;
    state.current_tunnel.clone()
};
```

After any successful resume, reacquire the lock only to store `state.current_tunnel = Some(resumed.clone())`. Do not call `close_listener_on_tunnel`, `resume_listen_session_inner`, or `close_tunnel_generation` while holding `state`.

- [ ] **Step 4: Verify no network await remains under state lock.**

Run: `sed -n '750,940p' crates/remote-exec-broker/src/port_forward/supervisor.rs`
Expected: lock scopes are short blocks, and awaited tunnel operations are outside them.

- [ ] **Step 5: Run focused port-forward tests.**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: C++ port-forward integration tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/supervisor.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "fix: avoid listen session lock during tunnel io"
```

### Task 9: Always Tear Down C++ Port Tunnel State On Unknown Exceptions

**Finding:** `#7`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-port-tunnel`
  - `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing tests + minimal defensive code
Reason: Forcing a non-`std::exception` through the private dispatch loop would require test-only hooks. The fix is small and deterministic.

- [ ] **Step 1: Add catch-all teardown path.**

In `PortTunnelConnection::run()`, after the existing `catch (const std::exception& ex)` block, add:

```cpp
    } catch (...) {
        close_mode = PortTunnelCloseMode::TerminalFailure;
        send_terminal_error(0U, "invalid_port_tunnel", "unknown port tunnel failure");
    }
```

Keep the existing `close_current_session(close_mode);` and `close_transport_owned_state();` after the try/catch so both exception paths share teardown.

- [ ] **Step 2: Run C++ tunnel tests.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-port-tunnel`
Expected: port tunnel tests pass.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: POSIX check passes.

- [ ] **Step 3: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp
git commit -m "fix: tear down cpp tunnels after unknown exceptions"
```

### Task 10: Make Host TCP EOF And Broker Heartbeat ACK Delivery Reliable

**Finding:** `#8`

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward/tcp.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-broker/src/port_forward/tunnel.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host tunnel_tcp_eof`
  - `cargo test -p remote-exec-host tunnel_tcp_data_write_pressure_does_not_block_control_frames`
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** TDD for TCP EOF; existing pressure test for heartbeat
Reason: EOF/shutdown delivery is observable through bounded queues. Heartbeat delivery already has a pressure regression test in host; broker needs code inspection plus integration coverage.

- [ ] **Step 1: Add a host unit test for full writer queue EOF.**

In `crates/remote-exec-host/src/port_forward/mod.rs`, near existing `tunnel_tcp_eof` tests, add:

```rust
#[tokio::test]
async fn tunnel_tcp_eof_waits_for_full_writer_queue() {
    let state = test_state();
    let (tx, mut rx) = tokio::sync::mpsc::channel(super::TCP_WRITE_QUEUE_FRAMES);
    for _ in 0..super::TCP_WRITE_QUEUE_FRAMES {
        tx.send(TcpWriteCommand::Data(Vec::new())).await.unwrap();
    }
    let stream_cancel = tokio_util::sync::CancellationToken::new();
    let tunnel = test_connect_tunnel_with_tcp_stream(state, 1, tx, stream_cancel.clone()).await;

    let eof = tokio::spawn({
        let tunnel = tunnel.clone();
        async move { tunnel_tcp_eof(&tunnel, 1).await.unwrap() }
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!eof.is_finished(), "EOF should wait instead of dropping shutdown");

    while matches!(rx.recv().await, Some(TcpWriteCommand::Data(_))) {
        break;
    }
    tokio::time::timeout(Duration::from_secs(1), eof)
        .await
        .expect("EOF should complete after queue has capacity")
        .unwrap();
    while let Some(command) = rx.recv().await {
        if matches!(command, TcpWriteCommand::Shutdown) {
            return;
        }
    }
    panic!("expected shutdown command");
}
```

If `test_connect_tunnel_with_tcp_stream` does not exist, extract it from the existing manual tunnel setup in the same test module.

- [ ] **Step 2: Run the failing host test.**

Run: `cargo test -p remote-exec-host tunnel_tcp_eof_waits_for_full_writer_queue`
Expected: test times out or fails because `try_send` drops the shutdown.

- [ ] **Step 3: Replace `try_send` for TCP shutdown with awaited send plus timeout.**

Add a small helper in `tcp.rs`:

```rust
const TCP_CONTROL_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

async fn send_tcp_shutdown(writer: &TcpWriterHandle) -> bool {
    tokio::time::timeout(
        TCP_CONTROL_SEND_TIMEOUT,
        writer.tx.send(TcpWriteCommand::Shutdown),
    )
    .await
    .is_ok_and(Result::is_ok)
}
```

Use `send_tcp_shutdown(&writer).await` in both listen-owned and transport-owned branches of `tunnel_tcp_eof`. Clear cancel only when it returns `true`.

- [ ] **Step 4: Make broker heartbeat ACK use the normal reliable send path.**

In `crates/remote-exec-broker/src/port_forward/tunnel.rs`, replace:

```rust
let _ = reader_tx.try_send(QueuedFrame { frame: ack, charge: 0 });
```

with:

```rust
if reader_tx.send(QueuedFrame { frame: ack, charge: 0 }).await.is_err() {
    return;
}
```

This is safe because the reader task is already async, the ACK has zero byte-budget charge, and heartbeat ACKs are control traffic.

- [ ] **Step 5: Run focused tests.**

Run: `cargo test -p remote-exec-host tunnel_tcp_eof`
Expected: EOF tests pass.

Run: `cargo test -p remote-exec-host tunnel_tcp_data_write_pressure_does_not_block_control_frames`
Expected: pressure test passes.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: broker forwarding tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-host/src/port_forward crates/remote-exec-broker/src/port_forward/tunnel.rs
git commit -m "fix: deliver port tunnel control frames reliably"
```

### Task 11: Make Host Tunnel Open Mode Transition Atomic

**Finding:** `#9`

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host concurrent_tunnel_open`
  - `cargo test -p remote-exec-host port_tunnel`

**Testing approach:** TDD
Reason: The race is a state transition invariant and can be tested by driving concurrent open handlers.

- [ ] **Step 1: Add a concurrent open test.**

In `crates/remote-exec-host/src/port_forward/mod.rs`, add:

```rust
#[tokio::test]
async fn concurrent_tunnel_open_allows_only_one_mode() {
    let state = test_state();
    let (broker_side, daemon_side) = tokio::io::duplex(4096);
    tokio::spawn(serve_tunnel(state, daemon_side));
    let (mut read, mut write) = tokio::io::split(broker_side);
    write_preface(&mut write).await.unwrap();

    let listen_open = json_frame(
        FrameType::TunnelOpen,
        0,
        serde_json::json!({
            "version": 4,
            "role": "listen",
            "protocol": "tcp",
            "forward_id": "forward-a",
            "generation": 1
        }),
    );
    let connect_open = json_frame(
        FrameType::TunnelOpen,
        0,
        serde_json::json!({
            "version": 4,
            "role": "connect",
            "protocol": "tcp",
            "forward_id": "forward-a",
            "generation": 1
        }),
    );
    write_frame(&mut write, &listen_open).await.unwrap();
    write_frame(&mut write, &connect_open).await.unwrap();

    let mut ready_count = 0;
    let mut error_count = 0;
    for _ in 0..2 {
        let frame = read_frame(&mut read).await.unwrap();
        match frame.frame_type {
            FrameType::TunnelReady => ready_count += 1,
            FrameType::Error => error_count += 1,
            other => panic!("unexpected frame: {other:?}"),
        }
    }
    assert_eq!(ready_count, 1);
    assert_eq!(error_count, 1);
}
```

If the test cannot create true concurrency through one serialized read loop, add unit coverage around a new helper `claim_tunnel_mode(&TunnelState, TunnelMode)`.

- [ ] **Step 2: Run the focused test.**

Run: `cargo test -p remote-exec-host concurrent_tunnel_open`
Expected: test exposes duplicate-open behavior or passes as characterization for serialized read-loop behavior. Continue with the helper refactor either way because the two-lock pattern remains fragile.

- [ ] **Step 3: Add a single-guard claim helper.**

In `tunnel.rs`, add:

```rust
async fn claim_tunnel_mode(
    tunnel: &Arc<TunnelState>,
    mode: TunnelMode,
) -> Result<(), HostRpcError> {
    let mut open_mode = tunnel.open_mode.lock().await;
    if !matches!(*open_mode, TunnelMode::Unopened) {
        return Err(rpc_error(
            RpcErrorCode::PortTunnelAlreadyAttached,
            "port tunnel is already open",
        ));
    }
    *open_mode = mode;
    Ok(())
}
```

Use it in `tunnel_open_connect`.

For `tunnel_open_listen`, do all fallible awaits that do not require claiming the tunnel first, then call `claim_tunnel_mode` exactly once immediately before sending `TunnelReady`. If the code needs session attachment before ready, split it so `TunnelMode::Listen { .. }` is assigned under the same guard and no second open can pass between check and assignment.

- [ ] **Step 4: Run host port tunnel tests.**

Run: `cargo test -p remote-exec-host port_tunnel`
Expected: host port-forward unit tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-host/src/port_forward/tunnel.rs crates/remote-exec-host/src/port_forward/mod.rs
git commit -m "fix: claim host tunnel mode atomically"
```

### Task 12: Release All TCP Active Stream Budgets After Listen Cleanup Failures

**Finding:** `#10`

**Files:**
- Modify: `crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
- Modify: `crates/remote-exec-broker/tests/mcp_forward_ports.rs` if a practical budget regression test is added
- Test/Verify:
  - `cargo test -p remote-exec-broker --test mcp_forward_ports`

**Testing approach:** existing tests + focused unit if seam is practical
Reason: The code path depends on tunnel close failures; if a compact unit seam is not available, code review plus full forwarding regression is acceptable.

- [ ] **Step 1: Refactor cleanup to accumulate the first failure.**

Change `close_active_tcp_listen_streams` to keep closing every stream:

```rust
let mut first_error = None;
for (_, mut stream) in streams {
    release_pending_budget(&mut state.pending_budget, &mut stream);
    if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
        let classified = classify_transport_failure(
            err,
            "closing tcp listen stream after connect tunnel loss",
            TunnelRole::Listen,
        )
        .map(|_| ());
        if first_error.is_none() {
            first_error = Some(classified);
        }
    }
}
runtime
    .record_dropped_streams_and_release_active(dropped_count)
    .await;
if let Some(result) = first_error {
    result?;
}
Ok(())
```

Adjust exact types to match `classify_transport_failure` return type. The important invariant is that `record_dropped_streams_and_release_active` always runs after `streams` is taken.

- [ ] **Step 2: Inspect for other early returns in the same cleanup path.**

Run: `sed -n '520,550p' crates/remote-exec-broker/src/port_forward/tcp_bridge.rs`
Expected: no early return exists inside the stream loop before active budgets are released.

- [ ] **Step 3: Run broker forwarding tests.**

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: tests pass.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-broker/src/port_forward/tcp_bridge.rs crates/remote-exec-broker/tests/mcp_forward_ports.rs
git commit -m "fix: release tcp budgets after cleanup failures"
```

### Task 13: Cancel Host Session Expiry On Reattach

**Finding:** `#11`

**Files:**
- Modify: `crates/remote-exec-host/src/port_forward/session.rs`
- Modify: `crates/remote-exec-host/src/port_forward/tunnel.rs`
- Modify: `crates/remote-exec-host/src/port_forward/mod.rs`
- Modify: `crates/remote-exec-host/src/port_forward/types.rs` if `SessionState` fields move there
- Test/Verify:
  - `cargo test -p remote-exec-host reattached_session_is_not_removed_by_stale_expiry`
  - `cargo test -p remote-exec-host port_tunnel`

**Testing approach:** TDD
Reason: The race is a specific stale-timer behavior that can be reproduced with the existing shortened test timings.

- [ ] **Step 1: Add a stale-expiry regression test.**

In `crates/remote-exec-host/src/port_forward/mod.rs`, add:

```rust
#[tokio::test]
async fn reattached_session_is_not_removed_by_stale_expiry() {
    let state = test_state();
    let listen_endpoint = free_loopback_endpoint();
    let (_bound_endpoint, session_id) =
        open_resumable_tcp_listener(&state, &listen_endpoint).await;

    let _resumed = resume_session(&state, &session_id, TunnelForwardProtocol::Tcp).await;
    tokio::time::sleep(Duration::from_millis(250)).await;

    assert!(
        state.port_forward_sessions.get(&session_id).await.is_some(),
        "stale expiry task must not remove a reattached session"
    );
}
```

Use the existing helper names in the module; if `state.port_forward_sessions` is private under `AppState`, add a test helper near `wait_until_session_removed`.

- [ ] **Step 2: Run the failing test.**

Run: `cargo test -p remote-exec-host reattached_session_is_not_removed_by_stale_expiry`
Expected: test fails if the old detached timer removes the reattached session.

- [ ] **Step 3: Store and cancel expiry handles.**

Add this field to `SessionState`:

```rust
pub(super) expiry_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
```

Initialize it in `new_session`.

In `attach_session_to_tunnel`, take and abort any existing `expiry_task` before clearing `resume_deadline`:

```rust
if let Some(task) = session.expiry_task.lock().await.take() {
    task.abort();
}
```

In `schedule_session_expiry`, replace the discarded spawn with:

```rust
let handle = tokio::spawn(async move {
    tokio::time::sleep(timings().resume_timeout).await;
    if session.is_expired().await && session.current_attachment().await.is_none() {
        store.remove(&session.id).await;
        session.close_retained_resources().await;
        session.root_cancel.cancel();
    }
});
let mut expiry_task = session.expiry_task.lock().await;
if let Some(existing) = expiry_task.take() {
    existing.abort();
}
*expiry_task = Some(handle);
```

Be careful with ownership: clone `session` for the task and keep the original for storing the handle.

- [ ] **Step 4: Run host port-forward tests.**

Run: `cargo test -p remote-exec-host reattached_session_is_not_removed_by_stale_expiry`
Expected: new test passes.

Run: `cargo test -p remote-exec-host port_tunnel`
Expected: all host port-forward tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-host/src/port_forward
git commit -m "fix: cancel stale port tunnel expiry tasks"
```

### Task 14: Resolve Windows Private-Key Permission Follow-Up

**Finding:** `#12`

**Files:**
- Modify: `crates/remote-exec-pki/src/write.rs`
- Modify: `README.md`
- Modify: `crates/remote-exec-admin/tests/dev_init.rs` if CLI output/docs mention generated key handling
- Test/Verify:
  - `cargo test -p remote-exec-pki`
  - `cargo test -p remote-exec-admin --test dev_init`

**Testing approach:** platform-aware documentation plus Unix regression
Reason: This workspace currently has no Windows ACL dependency. Without adding and validating a Windows ACL implementation on Windows CI in this task, the truthful fix is to document the limitation explicitly and keep Unix behavior tested.

- [ ] **Step 1: Replace the hidden non-Unix code comment with explicit API documentation.**

In `crates/remote-exec-pki/src/write.rs`, add a private helper:

```rust
#[cfg(not(unix))]
fn note_private_key_acl_limitation(mode: u32) {
    let _ = mode;
    tracing::warn!(
        "remote-exec-pki cannot harden private-key ACLs on this platform; restrict the output directory permissions before sharing generated keys"
    );
}
```

Do not add `tracing` just for this if `remote-exec-pki` does not already depend on it. If adding logging would introduce a new dependency, keep this as a code comment and document it in README instead:

```rust
// Non-Unix builds do not currently apply a Windows ACL. The CLI documentation
// tells operators to generate into a private directory on those platforms.
let _ = mode;
```

- [ ] **Step 2: Document the Windows limitation in README.**

Near the certificate bootstrap section in `README.md`, add:

```markdown
- On Unix, generated private-key files are written with `0600` permissions. On Windows, this tool does not currently rewrite file ACLs; generate certificate bundles in a private directory and apply the desired Windows ACLs before sharing the directory with other users.
```

- [ ] **Step 3: Keep existing Unix permission regression.**

Run: `cargo test -p remote-exec-pki write_text_file_sets_key_permissions_after_rename`
Expected: Unix permission test passes on Unix hosts; skipped on non-Unix.

- [ ] **Step 4: Run PKI/admin tests.**

Run: `cargo test -p remote-exec-pki`
Expected: PKI tests pass.

Run: `cargo test -p remote-exec-admin --test dev_init`
Expected: admin dev-init tests pass.

- [ ] **Step 5: Commit.**

```bash
git add crates/remote-exec-pki/src/write.rs README.md crates/remote-exec-admin/tests/dev_init.rs
git commit -m "docs: clarify windows private key acl limitation"
```

### Task 15: Phase A Final Verification

**Files:**
- Test/Verify only

**Testing approach:** full focused gate
Reason: Phase A touches Rust transfer, port-forwarding, broker/daemon config, C++ daemon transfer/tunnel code, docs, and PKI.

- [ ] **Step 1: Format Rust.**

Run: `cargo fmt --all --check`
Expected: formatting check passes.

- [ ] **Step 2: Run focused Rust tests.**

Run: `cargo test -p remote-exec-host`
Expected: host tests pass.

Run: `cargo test -p remote-exec-daemon --test transfer_rpc`
Expected: daemon transfer tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_transfer`
Expected: broker transfer tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports`
Expected: broker Rust port-forward tests pass.

Run: `cargo test -p remote-exec-broker --test mcp_forward_ports_cpp`
Expected: broker C++ port-forward tests pass.

Run: `cargo test -p remote-exec-pki`
Expected: PKI tests pass.

- [ ] **Step 3: Run C++ checks.**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: POSIX C++ checks pass.

Run: `make -C crates/remote-exec-daemon-cpp check-windows-xp`
Expected: Windows XP-compatible C++ checks pass.

If available on this host, also run:

Run: `bmake -C crates/remote-exec-daemon-cpp check-posix`
Expected: BSD make POSIX checks pass.

- [ ] **Step 4: Run workspace quality gate.**

Run: `cargo test --workspace`
Expected: workspace tests pass.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: clippy passes without warnings.

Run: `git diff --check`
Expected: no whitespace errors.

- [ ] **Step 5: Commit any final formatting/doc fixes if needed.**

If final verification required formatting or small doc corrections:

```bash
git add <changed-files>
git commit -m "chore: finalize phase a audit fixes"
```

If no files changed, do not create an empty commit.

