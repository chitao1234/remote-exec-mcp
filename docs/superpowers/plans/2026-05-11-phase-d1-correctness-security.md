# Phase D1 Correctness Security Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Resolve only Phase D1 correctness and security risks from `docs/CODE_AUDIT_ROUND3.md`, without planning or implementing D2-D4.

**Architecture:** Treat the audit as review input against the current tree, not as an automatically current source of truth. Most D1 work is in the standalone C++ daemon's POSIX process, transfer, socket, and tunnel paths; the one Rust runtime change is Unix process-group termination for `remote-exec-host` exec sessions. Keep changes focused, preserve the broker-owned public ID model, and do not change public MCP schemas.

**Tech Stack:** Rust 2024 workspace with Tokio, `std::process`, local `portable-pty`, and Unix `nix`/`libc` calls; standalone C++11 daemon with POSIX and Windows-compatible socket abstractions; existing Cargo integration tests and C++ make targets.

---

## Scope

Included from round-3 Phase D1:

- `#1`: C++ transfer import archive path traversal with `..` components.
- `#2`: C++ POSIX child calls `setenv` after `fork`.
- `#3`: C++ POSIX `pipe()` descriptors are not created with close-on-exec.
- `#4`: Rust exec termination is SIGKILL-only for pipe children and does not terminate process groups.
- `#5`: C++ socket read/write helpers narrow `size_t` to `int`.
- `#13`: C++ tunnel frame sending holds `writer_mutex_` across potentially long blocking sends.

Explicitly excluded from this plan: D2 lifecycle/timeouts, D3 observability/operability, and D4 test reliability. Do not add PTY resize, request IDs, metrics, daemon SIGTERM handling, zombie reaping, `LiveSession::Drop`, or test-infrastructure cleanups as part of this plan.

Current-state note: `validate_relative_archive_path` in `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp` already rejects `.` and `..` components, and `tests/test_transfer.cpp` already has a top-level `../escape.txt` rejection test. Task 2 below extends the regression coverage to nested traversal, dot components, duplicate separators, Windows separators, and GNU long-name traversal so the already-fixed behavior is locked down.

## File Structure

- `docs/superpowers/plans/2026-05-11-phase-d1-correctness-security.md`: this D1-only implementation plan.
- `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`: add transfer-import traversal regression coverage.
- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`: add CLOEXEC pipe creation; build child environment in the parent; resolve executable path in the parent; use `execve` in the child.
- `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`: add C++ POSIX exec environment and PATH-resolution regression coverage.
- `crates/remote-exec-host/src/exec/session/child.rs`: add Unix process-group termination helper and graceful SIGTERM-to-SIGKILL escalation for pipe children.
- `crates/remote-exec-host/src/exec/session/spawn.rs`: start Unix pipe-mode commands in a new process group.
- `crates/remote-exec-host/src/exec/session/mod.rs`: add Rust Unix integration-style unit coverage for process-group cleanup.
- `crates/remote-exec-daemon-cpp/include/server_transport.h`: declare bounded socket transfer helpers.
- `crates/remote-exec-daemon-cpp/src/server_transport.cpp`: implement bounded `send`/`recv` chunks and use bounded send in `send_all_bytes`.
- `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`: use bounded receive in `read_exact`; move frame transmission to a dedicated writer thread so producers do not block under `writer_mutex_`.
- `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`: add `PortTunnelSender` queue, condition variable, and writer-thread state.
- `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`: add socket helper unit coverage.

---

### Task 1: Save The Phase D1 Plan

**Files:**
- Create: `docs/superpowers/plans/2026-05-11-phase-d1-correctness-security.md`
- Test/Verify: `git status --short docs/superpowers/plans/2026-05-11-phase-d1-correctness-security.md`

**Testing approach:** no new tests needed
Reason: This task creates the tracked plan artifact only. The repo already tracks many files under `docs/superpowers/plans`, so the new plan should follow that convention.

- [ ] **Step 1: Verify this plan file exists.**

Run: `test -f docs/superpowers/plans/2026-05-11-phase-d1-correctness-security.md`
Expected: command exits successfully.

- [ ] **Step 2: Review the plan heading and scope.**

Run: `sed -n '1,90p' docs/superpowers/plans/2026-05-11-phase-d1-correctness-security.md`
Expected: output names Phase D1 only, includes the required agentic-worker header, and explicitly excludes D2-D4.

- [ ] **Step 3: Commit.**

```bash
git add docs/superpowers/plans/2026-05-11-phase-d1-correctness-security.md
git commit -m "docs: plan phase d1 audit fixes"
```

### Task 2: Lock Down C++ Archive Traversal Rejection

**Finding:** D1 `#1`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`
- Modify only if the new tests fail: `crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp`
- Test/Verify: `make -C crates/remote-exec-daemon-cpp test-host-transfer`

**Testing approach:** characterization/regression test
Reason: The current validator already rejects `.` and `..` components. The right D1 action is to prove the behavior across bypass shapes that would have exploited the audited bug and keep future C++/Rust validator drift visible.

- [ ] **Step 1: Add an import rejection helper.**

Add this helper near `assert_directory_traversal_is_rejected` in `crates/remote-exec-daemon-cpp/tests/test_transfer.cpp`:

```cpp
static bool directory_import_rejects_path(const std::string& path) {
    const std::string archive = tar_with_single_file(path, "bad");
    const fs::path root = fs::temp_directory_path() / "remote-exec-cpp-transfer-path-reject";
    fs::remove_all(root);

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
        rejected =
            failure.message.find("archive path") != std::string::npos ||
            failure.message.find("escapes destination") != std::string::npos;
    }

    assert(!fs::exists(root / "escape.txt"));
    assert(!fs::exists(root / "dest" / "escape.txt"));
    return rejected;
}
```

- [ ] **Step 2: Expand traversal test cases.**

Replace `assert_directory_traversal_is_rejected` with:

```cpp
static void assert_directory_traversal_is_rejected() {
    assert(directory_import_rejects_path("../escape.txt"));
    assert(directory_import_rejects_path("foo/../../../etc/shadow"));
    assert(directory_import_rejects_path("safe/../escape.txt"));
    assert(directory_import_rejects_path("safe/./escape.txt"));
    assert(directory_import_rejects_path("safe//escape.txt"));
    assert(directory_import_rejects_path("safe\\..\\escape.txt"));
    assert(directory_import_rejects_path("safe\\.\\escape.txt"));

    std::string long_name_archive;
    append_gnu_long_name(&long_name_archive, "safe/../../escape.txt");
    append_tar_entry(&long_name_archive, "ignored", '0', "bad");
    finalize_tar(long_name_archive);

    const fs::path root =
        fs::temp_directory_path() / "remote-exec-cpp-transfer-long-name-traversal";
    fs::remove_all(root);
    bool rejected = false;
    try {
        (void)import_path(
            long_name_archive,
            TransferSourceType::Directory,
            (root / "dest").string(),
            "replace",
            true
        );
    } catch (const TransferFailure& failure) {
        rejected =
            failure.message.find("archive path") != std::string::npos ||
            failure.message.find("escapes destination") != std::string::npos;
    }
    assert(rejected);
    assert(!fs::exists(root / "escape.txt"));
    assert(!fs::exists(root / "dest" / "escape.txt"));
}
```

- [ ] **Step 3: Run focused C++ transfer verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-transfer`
Expected: test passes. If it fails, fix `validate_relative_archive_path` so it normalizes `\` to `/`, strips only leading `./`, rejects absolute paths and drive prefixes, splits on `/`, and rejects any empty, `.`, or `..` component.

- [ ] **Step 4: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/tests/test_transfer.cpp crates/remote-exec-daemon-cpp/src/transfer_ops_import.cpp
git commit -m "test: lock down cpp archive traversal rejection"
```

### Task 3: Make C++ POSIX Exec Child Setup Async-Signal-Safe

**Findings:** D1 `#2`, D1 `#3`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-session-store`
  - `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** TDD for externally observable exec behavior, existing build coverage for fork-safety internals
Reason: The unsafe behavior is about which functions run after `fork` in a multithreaded process. A deterministic deadlock test would be brittle; keep behavior tests around environment/PATH semantics, then inspect and verify the implementation uses only parent-built data and `execve` in the child.

- [ ] **Step 1: Add POSIX behavior coverage for locale and PATH lookup.**

Add these POSIX includes near the top of `crates/remote-exec-daemon-cpp/tests/test_session_store.cpp`:

```cpp
#ifndef _WIN32
#include <cstdlib>
#include <sys/stat.h>
#include <unistd.h>
#endif
```

Add this function after `assert_posix_locale_and_late_output`:

```cpp
static void assert_posix_exec_uses_parent_built_environment_and_path(
    SessionStore& store,
    const fs::path& root,
    const std::string& shell,
    const YieldTimeConfig& yield_time
) {
#ifdef _WIN32
    (void)store;
    (void)root;
    (void)shell;
    (void)yield_time;
#else
    const fs::path bin_dir = root / "path-bin";
    fs::create_directories(bin_dir);
    const fs::path helper = bin_dir / "env-helper";
    write_text_file(
        helper,
        "#!/bin/sh\n"
        "printf '%s|%s|%s\\n' \"$LC_ALL\" \"$LANG\" \"$TERM\"\n"
    );
    chmod(helper.c_str(), 0755);

    const char* old_path_raw = std::getenv("PATH");
    const bool had_old_path = old_path_raw != NULL;
    const std::string old_path = had_old_path ? old_path_raw : "";
    const std::string new_path = bin_dir.string() + ":" + old_path;
    assert(setenv("PATH", new_path.c_str(), 1) == 0);

    const Json pipe_response = start_test_command(
        store,
        "env-helper",
        root.string(),
        shell,
        false,
        5000UL,
        DEFAULT_MAX_OUTPUT_TOKENS,
        yield_time,
        64UL
    );
    assert(pipe_response.at("exit_code").get<int>() == 0);
    assert(pipe_response.at("output").get<std::string>() == "C.UTF-8|C.UTF-8|\n");

    if (process_session_supports_pty()) {
        const Json pty_response = start_test_command(
            store,
            "env-helper",
            root.string(),
            shell,
            true,
            5000UL,
            DEFAULT_MAX_OUTPUT_TOKENS,
            yield_time,
            64UL
        );
        assert(pty_response.at("exit_code").get<int>() == 0);
        assert(
            normalize_output(pty_response.at("output").get<std::string>()) ==
            "C.UTF-8|C.UTF-8|xterm-256color\n"
        );
    }

    if (had_old_path) {
        assert(setenv("PATH", old_path.c_str(), 1) == 0);
    } else {
        assert(unsetenv("PATH") == 0);
    }
#endif
}
```

Call it from `main()` immediately after `assert_posix_locale_and_late_output(store, root, shell, yield_time);`:

```cpp
    assert_posix_exec_uses_parent_built_environment_and_path(store, root, shell, yield_time);
```

- [ ] **Step 2: Run the behavior test before implementation.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
Expected: currently passes or fails depending on existing behavior. Passing does not mean the audit item is fixed; continue because this test protects semantics while replacing `setenv`/`execvp`.

- [ ] **Step 3: Add close-on-exec pipe creation.**

In `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`, replace `create_posix_pipe` with an implementation that sets `FD_CLOEXEC` before returning:

```cpp
void set_cloexec_or_throw(int fd, const char* label) {
    const int flags = fcntl(fd, F_GETFD, 0);
    if (flags < 0) {
        throw std::runtime_error(std::string(label) + " fcntl(F_GETFD) failed: " + std::strerror(errno));
    }
    if (fcntl(fd, F_SETFD, flags | FD_CLOEXEC) != 0) {
        throw std::runtime_error(std::string(label) + " fcntl(F_SETFD) failed: " + std::strerror(errno));
    }
}

PosixPipePair create_posix_pipe(const char* label) {
    int fds[2];
#ifdef __linux__
    if (pipe2(fds, O_CLOEXEC) != 0) {
        throw std::runtime_error(std::string(label) + " failed: " + std::strerror(errno));
    }
#else
    if (pipe(fds) != 0) {
        throw std::runtime_error(std::string(label) + " failed: " + std::strerror(errno));
    }
    try {
        set_cloexec_or_throw(fds[0], label);
        set_cloexec_or_throw(fds[1], label);
    } catch (...) {
        close(fds[0]);
        close(fds[1]);
        throw;
    }
#endif
    PosixPipePair pair;
    pair.read_end.reset(fds[0]);
    pair.write_end.reset(fds[1]);
    return pair;
}
```

- [ ] **Step 4: Build `argv`, `envp`, and executable path in the parent.**

Add these helpers above `exec_shell_child`:

```cpp
extern char** environ;

struct ExecEnvironment {
    std::vector<std::string> values;
    std::vector<char*> pointers;

    void refresh_pointers() {
        pointers.clear();
        pointers.reserve(values.size() + 1U);
        for (std::size_t i = 0; i < values.size(); ++i) {
            pointers.push_back(const_cast<char*>(values[i].c_str()));
        }
        pointers.push_back(NULL);
    }
};

bool env_key_matches(const std::string& entry, const char* key) {
    const std::size_t key_len = std::strlen(key);
    return entry.size() > key_len && entry.compare(0, key_len, key) == 0 && entry[key_len] == '=';
}

void upsert_env_value(std::vector<std::string>* values, const std::string& assignment) {
    const std::size_t equals = assignment.find('=');
    const std::string key = equals == std::string::npos ? assignment : assignment.substr(0, equals);
    for (std::size_t i = 0; i < values->size(); ++i) {
        if (env_key_matches((*values)[i], key.c_str())) {
            (*values)[i] = assignment;
            return;
        }
    }
    values->push_back(assignment);
}

ExecEnvironment build_exec_environment_values(bool tty) {
    ExecEnvironment env;
    for (char** current = environ; current != NULL && *current != NULL; ++current) {
        env.values.push_back(*current);
    }
    upsert_env_value(&env.values, "LC_ALL=C.UTF-8");
    upsert_env_value(&env.values, "LANG=C.UTF-8");
    if (tty) {
        bool has_term = false;
        for (std::size_t i = 0; i < env.values.size(); ++i) {
            if (env_key_matches(env.values[i], "TERM")) {
                has_term = true;
                break;
            }
        }
        if (!has_term) {
            env.values.push_back("TERM=xterm-256color");
        }
    }
    return env;
}

bool is_path_like_command(const std::string& command) {
    return command.find('/') != std::string::npos;
}

std::string path_env_from(const ExecEnvironment& env) {
    for (std::size_t i = 0; i < env.values.size(); ++i) {
        if (env_key_matches(env.values[i], "PATH")) {
            return env.values[i].substr(5);
        }
    }
    return "/bin:/usr/bin";
}

std::string resolve_exec_path(const std::string& program, const ExecEnvironment& env) {
    if (program.empty() || is_path_like_command(program)) {
        return program;
    }
    const std::string path = path_env_from(env);
    std::string current;
    for (std::size_t i = 0; i <= path.size(); ++i) {
        if (i != path.size() && path[i] != ':') {
            current.push_back(path[i]);
            continue;
        }
        const std::string dir = current.empty() ? "." : current;
        const std::string candidate = dir + "/" + program;
        if (access(candidate.c_str(), X_OK) == 0) {
            return candidate;
        }
        current.clear();
    }
    return program;
}

std::vector<char*> build_exec_argv(const std::vector<std::string>& argv) {
    std::vector<char*> exec_argv;
    exec_argv.reserve(argv.size() + 1U);
    for (std::size_t i = 0; i < argv.size(); ++i) {
        exec_argv.push_back(const_cast<char*>(argv[i].c_str()));
    }
    exec_argv.push_back(NULL);
    return exec_argv;
}

```

Also add `#include <algorithm>` at the top if it is not already present.

- [ ] **Step 5: Replace child `setenv` and `execvp` with parent-built `execve`.**

Change `exec_shell_child` signature to:

```cpp
void exec_shell_child(
    const std::vector<char*>& exec_argv,
    const std::string& executable_path,
    const ExecEnvironment& environment,
    const std::string& workdir
)
```

Replace the body with:

```cpp
    if (!workdir.empty() && chdir(workdir.c_str()) != 0) {
        _exit(126);
    }

    execve(
        executable_path.c_str(),
        const_cast<char* const*>(&exec_argv[0]),
        const_cast<char* const*>(&environment.pointers[0])
    );
    _exit(127);
```

In `ProcessSession::launch`, build these before `fork()`:

```cpp
    ExecEnvironment exec_environment = build_exec_environment_values(tty);
    exec_environment.refresh_pointers();
    const std::vector<char*> exec_argv = build_exec_argv(argv);
    const std::string executable_path = resolve_exec_path(argv[0], exec_environment);
```

Update both child branches to call:

```cpp
            exec_shell_child(exec_argv, executable_path, exec_environment, workdir);
```

- [ ] **Step 6: Run focused and POSIX verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-session-store`
Expected: session-store tests pass, including locale and PATH behavior.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: POSIX daemon and all host tests pass.

- [ ] **Step 7: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/process_session_posix.cpp crates/remote-exec-daemon-cpp/tests/test_session_store.cpp
git commit -m "fix: make cpp posix exec child setup fork safe"
```

### Task 4: Terminate Rust Unix Exec Process Groups Gracefully

**Finding:** D1 `#4`

**Files:**
- Modify: `crates/remote-exec-host/src/exec/session/spawn.rs`
- Modify: `crates/remote-exec-host/src/exec/session/child.rs`
- Modify: `crates/remote-exec-host/src/exec/session/mod.rs`
- Test/Verify:
  - `cargo test -p remote-exec-host exec_session_termination`
  - `cargo test -p remote-exec-daemon --test exec_rpc`

**Testing approach:** TDD on Unix
Reason: This is externally visible process lifecycle behavior: terminating a stored shell session should also terminate background descendants in its process group. A Unix-only host test can prove the cleanup without changing public RPC schemas.

- [ ] **Step 1: Add a Unix test for process-group cleanup.**

Append this test to the existing `#[cfg(test)] mod tests` in `crates/remote-exec-host/src/exec/session/mod.rs`:

```rust
    #[cfg(unix)]
    #[tokio::test]
    async fn exec_session_termination_kills_pipe_process_group_descendants() {
        use std::time::{Duration, Instant};

        use crate::config::ProcessEnvironment;

        let tempdir = tempfile::tempdir().expect("tempdir");
        let marker = tempdir.path().join("descendant-marker");
        let script = format!(
            "trap 'exit 0' TERM; (trap 'exit 0' TERM; while :; do touch {}; sleep 0.05; done) & echo ready; while :; do sleep 1; done",
            marker.display()
        );
        let cmd = vec![
            TEST_SHELL.to_string(),
            "-c".to_string(),
            script,
        ];

        let mut session = super::spawn::spawn(
            &cmd,
            tempdir.path(),
            false,
            &ProcessEnvironment::capture_current(),
        )
        .expect("session should spawn");

        let output = session
            .wait_for_output(Duration::from_secs(2))
            .await
            .expect("wait should succeed");
        match output {
            super::live::OutputWait::Chunk(chunk) => assert!(chunk.contains("ready")),
            other => panic!("expected ready output, got {other:?}"),
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        while !marker.exists() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(marker.exists(), "descendant did not create marker");

        session.terminate().await.expect("terminate should succeed");
        let modified_after_terminate = std::fs::metadata(&marker)
            .expect("marker metadata")
            .modified()
            .expect("marker modified time");
        tokio::time::sleep(Duration::from_millis(250)).await;
        let modified_later = std::fs::metadata(&marker)
            .expect("marker metadata after terminate")
            .modified()
            .expect("marker modified time after terminate");

        assert_eq!(
            modified_after_terminate, modified_later,
            "descendant kept running after session termination"
        );
    }
```

If `OutputWait` does not implement `Debug`, replace the panic arm with `panic!("expected ready output")`.

- [ ] **Step 2: Run the failing test.**

Run: `cargo test -p remote-exec-host exec_session_termination`
Expected: on Unix, the new test fails because the shell is killed but its background descendant keeps updating the marker.

- [ ] **Step 3: Start pipe sessions in a new Unix process group.**

In `crates/remote-exec-host/src/exec/session/spawn.rs`, import Unix `CommandExt`:

```rust
#[cfg(unix)]
use std::os::unix::process::CommandExt;
```

In `spawn_pipe`, before `command.spawn()?`, add:

```rust
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            let result = nix::libc::setpgid(0, 0);
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
```

In the workspace root `Cargo.toml`, add the `signal` and `process` features to the existing `nix` workspace dependency:

```toml
nix = { version = "0.31", default-features = false, features = ["user", "signal", "process"] }
```

- [ ] **Step 4: Add Unix process-group graceful termination.**

In `crates/remote-exec-host/src/exec/session/child.rs`, add these imports and helper:

```rust
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
fn terminate_unix_process_group(child: &mut std::process::Child) -> anyhow::Result<()> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if child.try_wait()?.is_some() {
        return Ok(());
    }

    let pgid = Pid::from_raw(child.id() as i32);
    let _ = killpg(pgid, Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = killpg(pgid, Signal::SIGKILL);
    let _ = child.wait()?;
    Ok(())
}
```

Change the `SessionChild::Pipe(child)` branch in `terminate()` to:

```rust
            SessionChild::Pipe(child) => {
                #[cfg(unix)]
                {
                    terminate_unix_process_group(child)?;
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                    let _ = child.try_wait()?;
                }
            }
```

Leave the PTY branch using `portable-pty`'s child killer for this D1 change. The local `portable-pty` Unix implementation already creates a new session for PTY children.

- [ ] **Step 5: Run focused and daemon exec verification.**

Run: `cargo test -p remote-exec-host exec_session_termination`
Expected: new process-group cleanup test passes.

Run: `cargo test -p remote-exec-daemon --test exec_rpc`
Expected: daemon exec RPC tests pass.

- [ ] **Step 6: Commit.**

```bash
git add Cargo.toml crates/remote-exec-host/src/exec/session/spawn.rs crates/remote-exec-host/src/exec/session/child.rs crates/remote-exec-host/src/exec/session/mod.rs
git commit -m "fix: terminate rust exec process groups"
```

### Task 5: Bound C++ Socket Transfer Chunk Sizes

**Finding:** D1 `#5`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/include/server_transport.h`
- Modify: `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
  - `make -C crates/remote-exec-daemon-cpp test-server-streaming`
  - `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`

**Testing approach:** focused helper tests + existing route/tunnel compile coverage
Reason: The dangerous cast is hard to exercise with a real multi-GB buffer in CI. A helper-level test can assert the chunk calculation around `INT_MAX`, while existing server/tunnel tests prove call sites still compile and behave.

- [ ] **Step 1: Add a bounded socket chunk helper declaration.**

In `crates/remote-exec-daemon-cpp/include/server_transport.h`, add after `receive_timeout_error`:

```cpp
std::size_t bounded_socket_io_size(std::size_t remaining);
int recv_bounded(SOCKET client, char* data, std::size_t remaining, int flags);
int send_bounded(SOCKET client, const char* data, std::size_t remaining, int flags);
```

- [ ] **Step 2: Add helper tests.**

In `crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp`, add `#include <climits>` and `#include <limits>` near the top, then add these assertions after the invalid-timeout-socket check:

```cpp
    assert(bounded_socket_io_size(0U) == 0U);
    assert(bounded_socket_io_size(1U) == 1U);
    assert(
        bounded_socket_io_size(static_cast<std::size_t>(INT_MAX)) ==
        static_cast<std::size_t>(INT_MAX)
    );
    assert(
        bounded_socket_io_size(static_cast<std::size_t>(INT_MAX) + 1U) ==
        static_cast<std::size_t>(INT_MAX)
    );
    assert(
        bounded_socket_io_size(std::numeric_limits<std::size_t>::max()) ==
        static_cast<std::size_t>(INT_MAX)
    );
```

- [ ] **Step 3: Run the focused failing test.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
Expected: compile fails because `bounded_socket_io_size` is not implemented yet.

- [ ] **Step 4: Implement bounded send/recv helpers.**

In `crates/remote-exec-daemon-cpp/src/server_transport.cpp`, add `#include <climits>`.

Add these functions near the socket error helpers:

```cpp
std::size_t bounded_socket_io_size(std::size_t remaining) {
    const std::size_t max_chunk = static_cast<std::size_t>(INT_MAX);
    return remaining > max_chunk ? max_chunk : remaining;
}

int recv_bounded(SOCKET client, char* data, std::size_t remaining, int flags) {
    return recv(client, data, static_cast<int>(bounded_socket_io_size(remaining)), flags);
}

int send_bounded(SOCKET client, const char* data, std::size_t remaining, int flags) {
    return send(client, data, static_cast<int>(bounded_socket_io_size(remaining)), flags);
}
```

In `send_all_bytes`, replace:

```cpp
        const int sent = send(
            client,
            data + offset,
            static_cast<int>(size - offset),
            0
        );
```

with:

```cpp
        const int sent = send_bounded(client, data + offset, size - offset, 0);
```

In `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`, replace the `recv` call in `PortTunnelConnection::read_exact` with:

```cpp
        const int received = recv_bounded(
            client_,
            reinterpret_cast<char*>(data + offset),
            size - offset,
            0
        );
```

- [ ] **Step 5: Run focused verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-host-server-transport`
Expected: server transport tests pass.

Run: `make -C crates/remote-exec-daemon-cpp test-server-streaming`
Expected: streaming server tests pass.

Run: `make -C crates/remote-exec-daemon-cpp test-port-tunnel-frame`
Expected: port tunnel frame tests pass.

- [ ] **Step 6: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/include/server_transport.h crates/remote-exec-daemon-cpp/src/server_transport.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp crates/remote-exec-daemon-cpp/tests/test_server_transport.cpp
git commit -m "fix: bound cpp socket io chunks"
```

### Task 6: Add A Dedicated C++ Tunnel Writer Queue

**Finding:** D1 `#13`

**Files:**
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
- Modify: `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`
- Test/Verify:
  - `make -C crates/remote-exec-daemon-cpp test-server-streaming`
  - `make -C crates/remote-exec-daemon-cpp check-posix`

**Testing approach:** existing integration-style C++ tunnel coverage + structural review
Reason: The desired change is concurrency structure: producers should enqueue complete encoded frames quickly, while one writer thread preserves byte-stream serialization. Existing tunnel tests provide regression coverage; the key acceptance checks are that producers do not call `send_all_bytes` while holding `writer_mutex_`, and only the writer thread performs socket writes.

- [ ] **Step 1: Capture the current structure.**

Run: `sed -n '330,365p' crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`
Expected: `PortTunnelSender::send_frame` holds `BasicLockGuard lock(writer_mutex_)` while calling `send_all_bytes`.

- [ ] **Step 2: Add writer queue state to `PortTunnelSender`.**

In `crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h`, add `#include <deque>` near the other standard includes.

Change the class declaration to support shared ownership by the writer thread:

```cpp
class PortTunnelSender : public std::enable_shared_from_this<PortTunnelSender> {
```

Change the private state of `PortTunnelSender` to include a queue and writer lifecycle fields:

```cpp
    struct QueuedFrame {
        QueuedFrame() : charge_value(0UL) {}
        QueuedFrame(std::vector<unsigned char> bytes_value, unsigned long charge)
            : bytes(std::move(bytes_value)), charge_value(charge) {}

        std::vector<unsigned char> bytes;
        unsigned long charge_value;
    };

    void writer_loop();
    bool ensure_writer_started_locked();
    bool enqueue_encoded_frame(std::vector<unsigned char> bytes, unsigned long charge_value);
    void release_queued_frame_reservation(unsigned long charge_value);
    void drain_queued_frame_reservations_locked();

    SOCKET client_;
    std::shared_ptr<PortTunnelService> service_;
    BasicMutex writer_mutex_;
    BasicCondVar writer_cond_;
    std::deque<QueuedFrame> writer_queue_;
    bool writer_started_;
    bool writer_shutdown_;
    std::atomic<bool> closed_;
    std::atomic<unsigned long> queued_bytes_;
```

Change `PortTunnelConnection` to store a shared sender:

```cpp
    std::shared_ptr<PortTunnelSender> sender_;
```

Update the constructor initializer:

```cpp
          sender_(new PortTunnelSender(client, service)),
```

Update the forwarding methods in `port_tunnel_transport.cpp`:

```cpp
void PortTunnelConnection::send_frame(const PortTunnelFrame& frame) {
    sender_->send_frame(frame);
}

bool PortTunnelConnection::send_data_frame_or_limit_error(const PortTunnelFrame& frame) {
    return sender_->send_data_frame_or_limit_error(*this, frame);
}

bool PortTunnelConnection::send_data_frame_or_drop_on_limit(const PortTunnelFrame& frame) {
    return sender_->send_data_frame_or_drop_on_limit(*this, frame);
}

bool PortTunnelConnection::closed() const {
    return sender_->closed();
}

void PortTunnelConnection::mark_closed() {
    sender_->mark_closed();
}
```

- [ ] **Step 3: Initialize writer state.**

Update `PortTunnelSender::PortTunnelSender` in `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp`:

```cpp
PortTunnelSender::PortTunnelSender(
    SOCKET client,
    const std::shared_ptr<PortTunnelService>& service
) : client_(client),
    service_(service),
    writer_started_(false),
    writer_shutdown_(false),
    closed_(false),
    queued_bytes_(0UL) {}
```

- [ ] **Step 4: Implement writer thread startup and loop.**

Add these methods in `crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp` after `PortTunnelSender::mark_closed`:

```cpp
bool PortTunnelSender::ensure_writer_started_locked() {
    if (writer_started_) {
        return true;
    }
#ifdef _WIN32
    struct Context {
        std::shared_ptr<PortTunnelSender> sender;
    };
    struct ThreadEntry {
        static unsigned __stdcall entry(void* raw_context) {
            std::unique_ptr<Context> context(static_cast<Context*>(raw_context));
            context->sender->writer_loop();
            return 0;
        }
    };
    std::unique_ptr<Context> context(new Context());
    context->sender = shared_from_this();
    HANDLE handle = begin_win32_thread(&ThreadEntry::entry, context.get());
    if (handle == NULL) {
        closed_.store(true);
        shutdown_socket(client_);
        return false;
    }
    context.release();
    CloseHandle(handle);
    writer_started_ = true;
    return true;
#else
    try {
        std::shared_ptr<PortTunnelSender> self = shared_from_this();
        std::thread([self]() { self->writer_loop(); }).detach();
        writer_started_ = true;
        return true;
    } catch (const std::exception& ex) {
        log_tunnel_exception("spawn tunnel writer thread", ex);
        closed_.store(true);
        shutdown_socket(client_);
        return false;
    } catch (...) {
        log_unknown_tunnel_exception("spawn tunnel writer thread");
        closed_.store(true);
        shutdown_socket(client_);
        return false;
    }
#endif
}

void PortTunnelSender::writer_loop() {
    for (;;) {
        QueuedFrame queued;
        {
            BasicLockGuard lock(writer_mutex_);
            while (writer_queue_.empty() && !writer_shutdown_ && !closed_.load()) {
                writer_cond_.wait(writer_mutex_);
            }
            if ((writer_shutdown_ || closed_.load()) && writer_queue_.empty()) {
                return;
            }
            queued = std::move(writer_queue_.front());
            writer_queue_.pop_front();
        }

        try {
            send_all_bytes(
                client_,
                reinterpret_cast<const char*>(queued.bytes.data()),
                queued.bytes.size()
            );
        } catch (const std::exception& ex) {
            log_tunnel_exception("send port tunnel frame", ex);
            release_queued_frame_reservation(queued.charge_value);
            closed_.store(true);
            shutdown_socket(client_);
            {
                BasicLockGuard lock(writer_mutex_);
                drain_queued_frame_reservations_locked();
                writer_cond_.broadcast();
            }
            return;
        }
        release_queued_frame_reservation(queued.charge_value);
    }
}
```

- [ ] **Step 5: Add enqueue and reservation cleanup helpers.**

Add these methods after `writer_loop`:

```cpp
bool PortTunnelSender::enqueue_encoded_frame(
    std::vector<unsigned char> bytes,
    unsigned long charge_value
) {
    BasicLockGuard lock(writer_mutex_);
    if (closed_.load()) {
        return false;
    }
    if (!ensure_writer_started_locked()) {
        return false;
    }
    writer_queue_.push_back(QueuedFrame(std::move(bytes), charge_value));
    writer_cond_.signal();
    return true;
}

void PortTunnelSender::release_queued_frame_reservation(unsigned long charge_value) {
    if (charge_value != 0UL) {
        release_data_frame_reservation(charge_value);
    }
}

void PortTunnelSender::drain_queued_frame_reservations_locked() {
    for (std::deque<QueuedFrame>::iterator it = writer_queue_.begin();
         it != writer_queue_.end();
         ++it) {
        release_queued_frame_reservation(it->charge_value);
    }
    writer_queue_.clear();
}
```

- [ ] **Step 6: Change producers to enqueue frames.**

Replace `PortTunnelSender::send_frame` with:

```cpp
void PortTunnelSender::send_frame(const PortTunnelFrame& frame) {
    std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
    (void)enqueue_encoded_frame(std::move(bytes), 0UL);
}
```

Update `mark_closed` so blocked writer waits wake up:

```cpp
void PortTunnelSender::mark_closed() {
    closed_.store(true);
    BasicLockGuard lock(writer_mutex_);
    writer_shutdown_ = true;
    drain_queued_frame_reservations_locked();
    writer_cond_.broadcast();
}
```

In `send_data_frame_or_limit_error`, replace the `try { send_frame(frame); ... }` block and the immediate release with:

```cpp
    try {
        std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        if (enqueue_encoded_frame(std::move(bytes), charge_value)) {
            return true;
        }
    } catch (const std::exception& ex) {
        log_tunnel_exception("queue limited port tunnel data frame", ex);
        release_data_frame_reservation(charge_value);
        throw;
    } catch (...) {
        log_unknown_tunnel_exception("queue limited port tunnel data frame");
        release_data_frame_reservation(charge_value);
        throw;
    }
    release_data_frame_reservation(charge_value);
    return false;
```

In `send_data_frame_or_drop_on_limit`, replace the `try { send_frame(frame); ... }` block and the immediate release with:

```cpp
    try {
        std::vector<unsigned char> bytes = encode_port_tunnel_frame(frame);
        if (enqueue_encoded_frame(std::move(bytes), charge_value)) {
            return true;
        }
    } catch (const std::exception& ex) {
        log_tunnel_exception("queue droppable port tunnel data frame", ex);
        release_data_frame_reservation(charge_value);
        throw;
    } catch (...) {
        log_unknown_tunnel_exception("queue droppable port tunnel data frame");
        release_data_frame_reservation(charge_value);
        throw;
    }
    release_data_frame_reservation(charge_value);
    return false;
```

The writer thread releases each nonzero `charge_value` after the queued frame is sent or discarded, so `max_tunnel_queued_bytes` remains a true queued-byte limit.

- [ ] **Step 7: Run tunnel/server verification.**

Run: `make -C crates/remote-exec-daemon-cpp test-server-streaming`
Expected: streaming server tests pass.

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: all POSIX C++ tests and daemon build pass.

- [ ] **Step 8: Commit.**

```bash
git add crates/remote-exec-daemon-cpp/src/port_tunnel_transport.cpp crates/remote-exec-daemon-cpp/src/port_tunnel_internal.h
git commit -m "fix: queue cpp tunnel frame writes"
```

### Task 7: Final D1 Verification

**Files:**
- Test/Verify only

**Testing approach:** targeted full D1 verification
Reason: D1 touches both Rust exec process handling and C++ daemon core paths. Run the focused commands before any broader workspace quality gate.

- [ ] **Step 1: Run C++ D1 verification.**

Run: `make -C crates/remote-exec-daemon-cpp check-posix`
Expected: all POSIX C++ host tests pass and the POSIX daemon binary builds.

- [ ] **Step 2: Run Rust exec verification.**

Run: `cargo test -p remote-exec-host exec_session_termination`
Expected: Rust host process-group termination test passes.

Run: `cargo test -p remote-exec-daemon --test exec_rpc`
Expected: daemon exec RPC tests pass.

- [ ] **Step 3: Run formatting/lint checks for touched languages.**

Run: `cargo fmt --all --check`
Expected: Rust formatting check passes.

Run: `cargo clippy --workspace --all-targets --all-features -- -D warnings`
Expected: Rust clippy check passes.

- [ ] **Step 4: Commit any verification-only adjustments.**

If formatting or linting required code changes:

```bash
git add crates/remote-exec-host crates/remote-exec-daemon-cpp
git commit -m "chore: finalize phase d1 audit fixes"
```

If no files changed, do not create an empty commit.
