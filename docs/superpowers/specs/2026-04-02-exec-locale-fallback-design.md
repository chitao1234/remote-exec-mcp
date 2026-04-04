# Exec Locale Fallback Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-daemon/src/exec/session.rs`
- `crates/remote-exec-daemon/tests/exec_rpc.rs`

## Goal

Preserve the current preference for a C-style locale with UTF-8 encoding, while making the daemon's environment normalization work on systems where `C.UTF-8` is not installed.

The resulting behavior should:

- prefer `C.UTF-8` whenever the target machine supports it
- otherwise prefer a hybrid locale shape with `LANG=C` plus UTF-8 character handling
- use `LC_ALL` only as a last resort to force UTF-8
- prefer English UTF-8 locales when selecting a non-`C.UTF-8` fallback
- keep PTY and pipe-backed child processes aligned

## Scope

This design covers only daemon-side locale normalization for child process execution.

In scope:

- replacing the fixed `C.UTF-8` locale overlay with a discovered host-supported strategy
- caching the chosen strategy in-process
- adding deterministic tests for locale selection and the child env seen by commands

Out of scope:

- shell resolution changes
- broker or proto changes
- session store changes
- output truncation changes
- PTY capability metadata changes

## Current Problem

The daemon currently injects:

- `LANG=C.UTF-8`
- `LC_CTYPE=C.UTF-8`
- `LC_ALL=C.UTF-8`

for every spawned command in `crates/remote-exec-daemon/src/exec/session.rs`.

That matches the documented Codex-inspired preference from `docs/local-system-tools.md`, but it assumes the host supports the literal locale name `C.UTF-8`.

On systems where `C.UTF-8` is unavailable, this can create a real runtime failure mode or at minimum a misleading portability signal. The project needs a deliberate fallback strategy rather than silently inheriting the host environment or inventing unsupported locale names.

## Decision Summary

### 1. Keep the existing non-locale env overlay unchanged

These entries stay fixed exactly as they are today:

- `NO_COLOR=1`
- `TERM=dumb`
- `COLORTERM=`
- `PAGER=cat`
- `GIT_PAGER=cat`
- `GH_PAGER=cat`
- `CODEX_CI=1`

Only the locale-related entries change.

### 2. Discover one host-supported locale strategy and cache it

The daemon should discover supported locale names from the host by running:

- `locale -a`

The result should be parsed once and turned into a cached strategy that is reused for later child spawns in the same process.

This avoids repeated probing and keeps PTY and pipe-backed exec behavior identical.

If locale discovery fails entirely, the daemon should fall back to:

- `LANG=C`
- no injected `LC_CTYPE`
- no injected `LC_ALL`

That keeps a safe C locale rather than inventing unsupported UTF-8 locale names.

### 3. Prefer these strategies in order

The chosen locale strategy should use this priority order:

1. exact `C.UTF-8`
2. exact `C.utf8`
3. hybrid locale:
   - `LANG=C`
   - `LC_CTYPE=<best installed English UTF-8 locale>`
4. last-resort UTF-8 forcing:
   - `LANG=C`
   - `LC_ALL=<best installed English UTF-8 locale>`
5. final safe fallback:
   - `LANG=C`
   - no injected `LC_CTYPE`
   - no injected `LC_ALL`

The preferred normal case remains the current one: a pure C locale with UTF-8 encoding.

### 4. Prefer English when selecting a non-C UTF-8 locale

When the daemon needs a non-`C.UTF-8` UTF-8 locale, it should rank candidates like this:

1. `en_US.UTF-8`
2. `en_US.utf8`
3. `en_GB.UTF-8`
4. `en_GB.utf8`
5. any `en_*.UTF-8` or `en_*.utf8`
6. any other UTF-8 locale

This keeps the locale fallback closer to English/C-style behavior while still preferring UTF-8 support.

### 5. Prefer `LANG=C` plus `LC_CTYPE` over `LC_ALL`

When `C.UTF-8` is unavailable, the daemon should first try to keep a true hybrid locale shape:

- `LANG=C`
- `LC_CTYPE=<utf8 locale>`

Only if the selected strategy explicitly requires it should the daemon inject:

- `LC_ALL=<utf8 locale>`

This follows the selected product rule:

- preserve a C-style language/messages baseline where possible
- force UTF-8 through `LC_CTYPE` first
- use `LC_ALL` only as the last resort

## Locale Strategy Shapes

The child process environment should end up in one of these shapes:

### Preferred direct UTF-8 C locale

- `LANG=C.UTF-8`
- `LC_CTYPE=C.UTF-8`
- `LC_ALL=C.UTF-8`

or the same shape with `C.utf8`.

### Preferred fallback hybrid locale

- `LANG=C`
- `LC_CTYPE=<selected utf8 locale>`
- no injected `LC_ALL`

### Last-resort UTF-8 forcing locale

- `LANG=C`
- `LC_ALL=<selected utf8 locale>`

### Final safe fallback

- `LANG=C`
- no injected `LC_CTYPE`
- no injected `LC_ALL`

## Code Boundaries

### `crates/remote-exec-daemon/src/exec/session.rs`

- replace the fixed locale entries in the current env overlay
- keep the shared spawn-time env application path for both PTY and pipe-backed processes
- delegate locale selection to a helper instead of hard-coding locale strings inline

### New helper module under `crates/remote-exec-daemon/src/exec/`

Add a focused locale helper module responsible for:

- invoking `locale -a`
- parsing supported locale names
- ranking candidates
- exposing a cached resolved locale strategy
- producing the final locale env pairs for child-process injection

This keeps the selection logic testable without making `session.rs` larger and more tangled.

### `crates/remote-exec-daemon/tests/exec_rpc.rs`

- update env-overlay assertions so they no longer assume literal `C.UTF-8`
- add deterministic tests for the effective child locale env in both pipe and PTY modes
- keep those tests independent from the host machine's real installed locale set

## Testing Strategy

### 1. Unit-style tests for locale ranking and parsing

Add tests around pure helper functions that accept discovered locale names and choose a strategy.

These tests should cover:

- `C.UTF-8` preferred over all other options
- `C.utf8` accepted when `C.UTF-8` is absent
- `LANG=C` plus `LC_CTYPE=en_US.UTF-8` chosen when no C-style UTF-8 locale exists
- `LC_ALL` selected only in the explicit last-resort path
- English UTF-8 locales preferred over non-English UTF-8 locales
- final fallback to `LANG=C` only when no UTF-8 locale exists

### 2. Deterministic exec RPC tests for child env

The exec RPC tests should verify the actual locale-related variables seen by spawned commands in:

- pipe mode
- PTY mode

To keep those tests deterministic, locale discovery should have a test seam so the tests can supply a synthetic discovered-locale set instead of depending on the host machine's real `locale -a` output.

That test seam is part of this design. Without it, the tests would become machine-dependent and would fail to prove the intended ranking logic.

## Rejected Alternatives

### Use the daemon's inherited `LANG` and `LC_*` environment as the fallback policy

This was rejected because it makes the daemon's behavior depend on ambient process configuration instead of enforcing the product's chosen locale policy.

The goal is to preserve an intentional hybrid locale shape, not to reuse whatever locale happened to be inherited by the daemon.

### Probe locale support through libc APIs

This was rejected because locale APIs are process-global and awkward in a concurrent daemon.

The simpler `locale -a` discovery path is easier to reason about and keeps the risk isolated to startup-time or first-use probing.

### Always use `LC_ALL=<utf8 locale>` when `C.UTF-8` is unavailable

This was rejected because it throws away the preferred hybrid locale behavior too early.

The selected product rule explicitly prefers:

- `LANG=C`
- `LC_CTYPE=<utf8 locale>`

with `LC_ALL` only as a last resort.

## Risks

### Low risk

- replacing literal `C.UTF-8` injection with `C.UTF-8` or `C.utf8` discovery
- adding pure ranking/parsing tests

### Low to medium risk

- getting the fallback selection wrong on machines with unusual locale inventories
- making tests nondeterministic if the locale discovery seam is not isolated properly

These risks are controlled by:

- explicit candidate ranking
- a cached resolved strategy
- tests that inject discovered locale names directly

## Success Criteria

This design is complete when:

- hosts that support `C.UTF-8` still receive the direct C UTF-8 locale overlay
- hosts without `C.UTF-8` receive the agreed fallback strategy
- `LANG=C` plus `LC_CTYPE=<utf8 locale>` is preferred before any `LC_ALL` fallback
- English UTF-8 locales outrank non-English locales when a non-C fallback is needed
- PTY and pipe-backed child processes see the same locale env policy
- tests prove the selection logic without depending on the real host locale inventory
