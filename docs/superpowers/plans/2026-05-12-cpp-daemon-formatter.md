# C++ Daemon Formatter Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Add a root `clang-format` configuration that defines the C++ daemon's formatting convention and apply it across the daemon's first-party C++ code.

**Requirements:**
- Add a formatter configuration at the repository root so editors and CLI tooling discover it automatically.
- Base the formatter on `LLVM` with a small set of explicit overrides rather than inventing a custom style from scratch.
- Treat the formatter as the formatting convention for `crates/remote-exec-daemon-cpp`.
- Allow include reordering as part of the convention.
- Apply the formatter to the daemon's first-party C++ source tree, including `src/`, `include/`, and `tests/`.
- Exclude `crates/remote-exec-daemon-cpp/third_party` from the formatting pass.
- Keep this slice config-and-format only; do not add make targets, CI enforcement, or unrelated documentation churn.

**Architecture:** Store one root `.clang-format` file so the convention is centralized and automatically inherited by the C++ daemon subtree. Use `BasedOnStyle: LLVM` with explicit overrides for the daemon's intended conventions such as 4-space indentation, attached braces, and include sorting. After the config is added, run the formatter only on the daemon's first-party C++ files so the checked-in tree matches the new convention immediately instead of deferring that churn to later feature work.

**Verification Strategy:** Confirm the formatter binary is available, run it over the targeted first-party daemon files, and review the resulting diff to ensure only formatting changes were introduced. Then run `make -C crates/remote-exec-daemon-cpp check-posix` as the primary build-and-test verification for the touched C++ code path.

**Assumptions / Open Questions:**
- `clang-format` is available in the local environment; if not, the implementation may need to stop after adding the config and report the missing tool.
- The root of the repo does not already contain a competing `.clang-format`; implementation should confirm before adding the file.
- Generated files or non-C++ assets are not part of the formatting pass even if they live under the daemon crate.

---

### Task 1: Add The Root Formatter Convention

**Intent:** Create a root `.clang-format` that defines the intended first-party C++ daemon style without expanding into build or CI automation.

**Relevant files/components:**
- Likely create: `.clang-format`
- Existing references: `crates/remote-exec-daemon-cpp/src/*.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/include/*.h`
- Existing references: `crates/remote-exec-daemon-cpp/tests/*.cpp`

**Notes / constraints:**
- Keep the configuration concise and explicit.
- Prefer overrides that align with the daemon's current style direction instead of sweeping opinionated changes.
- Include sorting should be enabled.
- Do not add ignore rules unless they are required by the formatter workflow; the implementation can exclude paths at invocation time.

**Verification:**
- Run: `clang-format --version`
- Run: `clang-format --style=file --dry-run crates/remote-exec-daemon-cpp/src/main.cpp`
- Expect: the config is discoverable from the repo root and accepted by the installed formatter.

- [ ] Confirm there is no existing root `.clang-format` that must be merged instead of replaced.
- [ ] Add the root formatter configuration with the approved style direction.
- [ ] Dry-run the formatter against a representative daemon file.
- [ ] Review the config for unintended repo-wide side effects.
- [ ] Commit.

### Task 2: Apply The Formatter To First-Party C++ Daemon Code

**Intent:** Bring the checked-in C++ daemon tree into compliance with the new convention immediately, while excluding vendored code.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-daemon-cpp/src/*.cpp`
- Likely modify: `crates/remote-exec-daemon-cpp/include/*.h`
- Likely modify: `crates/remote-exec-daemon-cpp/tests/*.cpp`
- Existing references: `crates/remote-exec-daemon-cpp/third_party/`

**Notes / constraints:**
- Exclude `crates/remote-exec-daemon-cpp/third_party` from the formatting pass.
- Keep the pass limited to first-party C++ sources and headers under the daemon crate.
- Review include reordering carefully because it is an intentional part of this change.
- Avoid opportunistic hand edits beyond formatting unless the formatter exposes a build break that must be corrected.

**Verification:**
- Run: `find crates/remote-exec-daemon-cpp/src crates/remote-exec-daemon-cpp/include crates/remote-exec-daemon-cpp/tests -type f \\( -name '*.cpp' -o -name '*.h' \\) -print0 | xargs -0 clang-format -i`
- Run: `git diff -- crates/remote-exec-daemon-cpp`
- Expect: the diff is formatting-only and does not touch `crates/remote-exec-daemon-cpp/third_party`.

- [ ] Identify the exact first-party file set to format.
- [ ] Run `clang-format` across the targeted daemon files.
- [ ] Confirm vendored `third_party` content was not modified.
- [ ] Review the diff for formatting-only changes and sensible include ordering.
- [ ] Commit.

### Task 3: Verify The Reformatted Tree Still Builds And Tests

**Intent:** Prove the new convention and formatting pass did not disturb the primary C++ daemon build path.

**Relevant files/components:**
- Existing references: `crates/remote-exec-daemon-cpp/Makefile`
- Existing references: `crates/remote-exec-daemon-cpp/mk/posix.mk`
- Existing references: `crates/remote-exec-daemon-cpp/src/`
- Existing references: `crates/remote-exec-daemon-cpp/include/`
- Existing references: `crates/remote-exec-daemon-cpp/tests/`

**Notes / constraints:**
- Keep verification focused on the main POSIX C++ path for this slice.
- CI or Windows-target verification can remain out of scope unless formatting exposes an unexpected platform-sensitive issue.
- Do not add new build targets as part of verification.

**Verification:**
- Run: `make -C crates/remote-exec-daemon-cpp check-posix`
- Expect: the first-party formatted tree still builds and passes the existing POSIX daemon checks.

- [ ] Run the focused POSIX C++ verification after formatting.
- [ ] Inspect any failures to determine whether they are formatting-related or pre-existing.
- [ ] Keep the change style-only unless verification reveals a necessary minimal fix.
- [ ] Record final verification status for the user.
- [ ] Commit.
