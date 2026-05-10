# C++ CI Parallelization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **For Codex subagent-driven execution:** Subagents cannot stream partial progress back to the controller while still running. The controller should assign each subagent a unique shared progress file and inspect that file during execution when visibility is needed.

**Goal:** Make the C++ CI job use available runner cores for build and test work instead of serial make invocations.

**Architecture:** Compute a dynamic make job count in the workflow. Linux runs POSIX and XP checks concurrently with separate `BUILD_DIR` values to avoid shared object/dependency-file races. Windows/MSYS2 runs the XP check with dynamic `-j`.

**Tech Stack:** GitHub Actions YAML, GNU make on Linux, GNU make under MSYS2 on Windows.

---

### Task 1: Linux C++ CI Parallel Checks

**Files:**
- Modify: `.github/workflows/ci.yml`

**Testing approach:** existing checks + command-shape validation.
Reason: This is CI orchestration; local verification should prove the shell script syntax and make targets remain valid.

- [x] Replace serial Linux C++ checks with a script that computes `jobs="$(nproc)"`.
- [x] Run POSIX and XP checks concurrently with isolated build directories:

```bash
jobs="$(nproc)"
make -j"${jobs}" -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-posix check-posix &
posix_pid="$!"
make -j"${jobs}" -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-windows-xp check-windows-xp &
xp_pid="$!"
posix_status=0
xp_status=0
wait "${posix_pid}" || posix_status="$?"
wait "${xp_pid}" || xp_status="$?"
if [ "${posix_status}" -ne 0 ] || [ "${xp_status}" -ne 0 ]; then
  exit 1
fi
```

- [x] Verify the command locally with the same shell script.

### Task 2: Windows C++ CI Parallel XP Check

**Files:**
- Modify: `.github/workflows/ci.yml`

**Testing approach:** syntax and local-equivalent validation.
Reason: The Windows-specific shell is only available in CI here; validate the POSIX shell syntax and keep it MSYS2-compatible.

- [x] Replace the Windows check command with an MSYS2-compatible dynamic core count:

```bash
jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)"
make -j"${jobs}" -C crates/remote-exec-daemon-cpp check-windows-xp
```

- [x] Verify that the YAML parses and the shell syntax is portable.

### Task 3: Verification and Commit

**Files:**
- Inspect: `.github/workflows/ci.yml`
- Inspect: `docs/superpowers/plans/2026-05-10-cpp-ci-parallelization.md`

**Testing approach:** focused CI command verification.
Reason: The behavior is CI-specific; local C++ make runs provide the best available proof.

- [x] Run `make -j"$(nproc)" -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-posix check-posix`.
- [x] Run `make -j"$(nproc)" -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-windows-xp check-windows-xp`.
- [x] Run `git diff --check`.
- [x] Commit with message `ci: parallelize cpp checks`.
