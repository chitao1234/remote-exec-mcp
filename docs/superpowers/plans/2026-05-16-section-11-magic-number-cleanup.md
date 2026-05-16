# Section 11 Magic Number Cleanup Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Keep commits non-empty and verify each task before committing.

**Goal:** Remove the verified section-11 magic-number smells that are truly low-risk readability problems, while rejecting proposed changes that would either conflate unrelated subsystems or expand into public configuration redesign.

**Requirements:**
- Limit this pass to the validated section-11 items from `docs/code-quality-audit-2026-05-16.md`.
- Keep behavior unchanged.
- Treat this as naming and local constant extraction cleanup, not a tuning-policy rewrite.
- Do not cargo-cult one subsystem's buffer sizes into another when the current code paths have different responsibilities.
- Do not widen daemon or broker public config surface in this pass just to make an internal limit tunable.

**Validated scope:**
- Accept `11.1` as a local naming cleanup in `exec/winpty.rs`.
- Accept the unnamed `8192` in `exec/session/spawn.rs` from `11.2`, but reject the audit's implied requirement to match the 64 KiB port-forward read buffer.
- Accept `11.4` as local constant extraction in the cited C++ files.
- Reject the `11.1` suggestion that the winpty timeout and port-forward connect timeout should be deduplicated; they are different subsystems with different failure domains.
- Defer the `11.3` “make transcript limit deploy-tunable” suggestion; that is a config-surface decision, not low-risk magic-number cleanup.

**Architecture note:** The right fix shape here is to replace unexplained literals with well-named local constants at the seam where the number matters. It is not to force one shared constant across unrelated domains unless the code already shares policy.

**Verification strategy:** Use focused Rust tests around exec behavior and the existing POSIX C++ test target for the touched daemon files. Finish with formatting checks after the final batch.

---

### Task 1: Name the Rust exec defaults that are currently inline literals

**Intent:** Replace the inline winpty default geometry, winpty startup timeout, and pipe-read buffer size with local constants that explain what each number means without altering behavior.

**Relevant files/components:**
- `crates/remote-exec-host/src/exec/winpty.rs`
- `crates/remote-exec-host/src/exec/session/spawn.rs`

**Notes / constraints:**
- Keep the winpty defaults local to the winpty backend; do not merge them with unrelated host or port-forward timeout settings.
- Keep the pipe session read buffer independent from the port-forward tunnel read buffer unless implementation finds an existing shared exec-specific policy surface.

**Verification:**
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Run: `cargo test -p remote-exec-broker --test mcp_exec`

- [ ] Extract named constants for winpty default rows, cols, and startup timeout
- [ ] Extract a named constant for the pipe output read buffer in `spawn.rs`
- [ ] Verify Rust exec behavior remains unchanged
- [ ] Commit

### Task 2: Replace the repeated C++ 4 KiB scratch buffers with named local constants

**Intent:** Remove the repeated anonymous `4096` buffers in the cited daemon files so the reader can tell whether the number is an I/O chunk size, a decode scratch buffer, or something else.

**Relevant files/components:**
- `crates/remote-exec-daemon-cpp/src/server_transport.cpp`
- `crates/remote-exec-daemon-cpp/src/process_session_posix.cpp`

**Notes / constraints:**
- Prefer per-file or per-responsibility constant names over one overly broad shared constant.
- It is acceptable if the server transport and process session files keep separate constant names because they handle different I/O paths.
- If a nearby threshold like `8192U` in `server_transport.cpp` reads as part of the same buffer-management policy, implementation may name it in the same pass.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`

- [ ] Extract named local constants for the repeated 4 KiB buffers
- [ ] Optionally name adjacent buffer-compaction thresholds if they are part of the same local policy
- [ ] Verify POSIX C++ checks remain clean
- [ ] Commit

### Task 3: Final section-11 sweep and explicit non-fix decisions

**Intent:** Close the section by formatting the touched files, rerunning a narrow sweep, and explicitly preserving the rejected/deferred items so the cleanup does not drift into policy redesign.

**Relevant files/components:**
- Touched Rust exec files
- Touched C++ daemon files

**Notes / constraints:**
- Do not expose `TRANSCRIPT_LIMIT_BYTES` as a new config option here.
- Do not force the pipe-reader buffer to 64 KiB just because port forwarding uses 64 KiB.

**Verification:**
- Run: `cargo fmt --all --check`
- Run: `cargo test -p remote-exec-broker --test mcp_exec`
- Run: `cargo test -p remote-exec-daemon --test exec_rpc`
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`

- [ ] Review the diff for accidental policy changes
- [ ] Run the final section-11 verification sweep
- [ ] Commit
