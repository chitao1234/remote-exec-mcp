# BSD CI Coverage Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Add periodic GitHub-hosted BSD CI coverage for FreeBSD, OpenBSD, NetBSD, and DragonFly BSD that exercises both the C++ daemon BSD build path and the full-feature Rust workspace tests.

**Requirements:**
- Use GitHub Actions, not a separate CI provider.
- Run BSD guests inside a VM action on a Linux GitHub-hosted runner rather than assuming native BSD runners exist.
- Cover `freebsd`, `openbsd`, `netbsd`, and `dragonflybsd`.
- Keep BSD coverage periodic and manually runnable, not part of the required push/pull-request CI path.
- Test both languages on each BSD guest:
  - C++ via the supported BSD make path
  - Rust via full-feature workspace tests
- Do not add BSD `clippy` jobs.
- Use `bmake` for the BSD C++ path, not GNU make.
- Reuse the BSD-built C++ daemon binary for Rust broker integration coverage through `REMOTE_EXEC_CPP_DAEMON`.
- Use `rustup` on FreeBSD and NetBSD.
- Use the platform-packaged Rust toolchain on OpenBSD and DragonFly BSD.
- Limit docs updates to live CI documentation in `README.md`; do not rewrite historical material under `docs/`.

**Architecture:** Add a separate periodic workflow, likely `.github/workflows/bsd-periodic.yml`, that runs on `ubuntu-latest` and uses `cross-platform-actions/action` to boot BSD guests. Each matrix entry carries the guest OS/version plus its package-install and Rust-setup policy so the workflow can keep one common execution shape while respecting OpenBSD/DragonFly Rust packaging constraints. Inside each guest, the workflow first runs `bmake -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-bsd check-posix`, then exports the resulting `remote-exec-daemon-cpp` path into `REMOTE_EXEC_CPP_DAEMON`, and finally runs `cargo test --workspace --all-features --locked`.

**Verification Strategy:** Validate the workflow syntax locally with `actionlint`, validate the documented BSD C++ path locally with `bmake -C crates/remote-exec-daemon-cpp check-posix`, and re-run the full-feature Rust workspace tests locally with the prebuilt C++ daemon path exported. The actual four-BSD guest execution is proven by the new scheduled/manual GitHub Actions workflow rather than by local host emulation.

**Assumptions / Open Questions:**
- The implementation should confirm the best currently supported `cross-platform-actions/action` version and the exact guest version strings for FreeBSD, OpenBSD, NetBSD, and DragonFly BSD.
- The implementation should confirm the exact package names needed per BSD guest for `git`, `curl`, `bmake`, a C++ compiler, and Rust where Rust comes from the guest package manager.
- The exact cron schedule is still open; if no stronger preference emerges during execution, choose one weekly UTC slot and keep `workflow_dispatch`.

---

### Task 1: Add A Periodic BSD Workflow Skeleton

**Intent:** Create the new GitHub Actions workflow with the correct trigger policy, matrix shape, and VM-action boundary without yet mixing in all guest commands.

**Relevant files/components:**
- Likely create: `.github/workflows/bsd-periodic.yml`
- Existing references: `.github/workflows/ci.yml`

**Notes / constraints:**
- Keep the new workflow separate from `ci.yml` so BSD coverage stays periodic and non-blocking for ordinary PR/push development.
- Use one matrix job with `fail-fast: false` rather than four copy-pasted jobs.
- The matrix should explicitly distinguish BSD guest toolchain policy, especially `rustup` vs packaged Rust.

**Verification:**
- Run: `actionlint .github/workflows/bsd-periodic.yml`
- Expect: the new workflow parses cleanly and uses valid GitHub Actions structure.

- [ ] Inspect the existing CI style and confirm the repo’s preferred workflow naming and formatting conventions
- [ ] Create the periodic/manual BSD workflow with a four-entry BSD matrix and GitHub-hosted Linux runner
- [ ] Encode guest metadata per BSD entry, including guest version and Rust setup policy
- [ ] Run workflow syntax validation
- [ ] Commit

### Task 2: Wire Guest Bootstrap And Full BSD Test Commands

**Intent:** Add the guest-side install/setup logic and the actual C++ and Rust commands that provide BSD coverage on every matrix entry.

**Relevant files/components:**
- Likely modify: `.github/workflows/bsd-periodic.yml`
- Existing references: `crates/remote-exec-daemon-cpp/README.md`
- Existing references: `AGENTS.md`
- Existing references: `crates/remote-exec-broker/tests/mcp_forward_ports_cpp.rs`

**Notes / constraints:**
- Use `bmake -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-bsd check-posix` for the BSD C++ path.
- Export `REMOTE_EXEC_CPP_DAEMON` to the BSD-built daemon binary before running Rust tests so broker integration coverage can consume the real C++ daemon.
- Keep Rust as `cargo test --workspace --all-features --locked`; do not add `clippy` or no-default-features coverage to BSD CI.
- FreeBSD and NetBSD should use `rustup`; OpenBSD and DragonFly BSD should use the guest package manager’s Rust package.

**Verification:**
- Run: `actionlint .github/workflows/bsd-periodic.yml`
- Expect: the guest bootstrap and command steps still parse correctly after the matrix and shell logic land.
- Run: `bmake -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-bsd check-posix`
- Expect: the supported BSD make path still passes locally.
- Run: `REMOTE_EXEC_CPP_DAEMON=$PWD/crates/remote-exec-daemon-cpp/build/ci-bsd/remote-exec-daemon-cpp cargo test --workspace --all-features --locked`
- Expect: local full-feature Rust tests still pass with an explicit prebuilt C++ daemon path.

- [ ] Confirm the exact guest package-install commands and Rust installation commands per BSD entry
- [ ] Add the shared guest-shell execution flow and the C++ build/test step
- [ ] Export the BSD-built daemon path into `REMOTE_EXEC_CPP_DAEMON` and run the full-feature Rust workspace tests
- [ ] Run focused local verification for workflow syntax, C++ BSD make, and Rust full-feature tests
- [ ] Commit

### Task 3: Update Live CI Documentation And Do A Final Workflow Sweep

**Intent:** Document the new periodic BSD coverage in the live README CI section and finish with a final verification pass.

**Relevant files/components:**
- Likely modify: `README.md`
- Likely inspect: `.github/workflows/bsd-periodic.yml`
- Existing references: `.github/workflows/ci.yml`

**Notes / constraints:**
- Keep the README update limited to live CI behavior and commands.
- Document that Linux/Windows remain in the main workflow, while BSD runs in periodic/manual VM-backed jobs.
- Mention the toolchain split clearly: FreeBSD/NetBSD use `rustup`; OpenBSD/DragonFly use packaged Rust.

**Verification:**
- Run: `actionlint .github/workflows/ci.yml .github/workflows/bsd-periodic.yml`
- Expect: both workflows lint cleanly together.
- Run: `bmake -C crates/remote-exec-daemon-cpp BUILD_DIR=build/ci-bsd check-posix`
- Expect: the documented BSD C++ path still passes after docs/workflow cleanup.
- Run: `REMOTE_EXEC_CPP_DAEMON=$PWD/crates/remote-exec-daemon-cpp/build/ci-bsd/remote-exec-daemon-cpp cargo test --workspace --all-features --locked`
- Expect: the full-feature Rust verification still passes with the same explicit daemon path.

- [ ] Update the README CI section to describe the periodic BSD workflow and its language coverage accurately
- [ ] Re-check the workflow for duplication, unclear matrix fields, or unnecessary branching
- [ ] Run the final workflow and local verification commands
- [ ] Commit any real follow-up changes; do not create an empty sweep commit
