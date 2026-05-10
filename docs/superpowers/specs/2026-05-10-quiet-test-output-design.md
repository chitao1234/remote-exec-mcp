# Quiet Test Output Design

## Problem

`cargo test --workspace` produces noisy stderr during normal passing runs. A
baseline capture showed 2,024 stderr lines, including 1,803 `INFO` lines and
109 `WARN` lines. Most output came from real broker child processes launched by
integration tests with production logging defaults; smaller C++ daemon and C++
host-test logs used the same pattern.

## Goals

- Keep production broker and daemon logging defaults unchanged.
- Make test-launched subprocesses quiet by default.
- Preserve useful stderr for assertion failures, panics, compiler output, and
  explicit diagnostics.
- Keep debugging opt-in through explicit environment variables.

## Design

Rust integration-test subprocess helpers apply a quiet test log default before
spawning the broker binary. The default is `REMOTE_EXEC_LOG=error`; it suppresses
normal `INFO` and expected negative-path `WARN` logs but still allows unexpected
errors through. A caller-provided `REMOTE_EXEC_LOG` or `RUST_LOG` remains
authoritative, and `REMOTE_EXEC_TEST_LOG` can override the quiet default for
ad-hoc investigation.

C++ integration tests apply the same subprocess rule to spawned C++ daemons and
streamable HTTP broker children. Standalone C++ make test recipes use a quiet
unit-test default of `REMOTE_EXEC_LOG=off`, while honoring explicit
`REMOTE_EXEC_LOG` and the same `REMOTE_EXEC_TEST_LOG` override.

The implementation does not redirect stderr to `/dev/null`; it reduces log
emission at the source so failure output is still visible.

## Verification

Focused checks cover noisy Rust broker tests and C++ host tests. The final check
captures `cargo test --workspace` stdout/stderr and compares line counts against
the baseline.
