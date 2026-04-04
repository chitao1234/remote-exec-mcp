# PTY Test Cleanup Design

Status: approved design captured in writing

Date: 2026-04-02

References:

- `crates/remote-exec-daemon/tests/exec_rpc.rs`
- `crates/remote-exec-daemon/src/exec/mod.rs`
- `crates/remote-exec-daemon/src/exec/session.rs`

## Goal

Make the daemon PTY exec tests less OS-sensitive while preserving meaningful behavioral coverage for `write_stdin`.

The resulting test changes should:

- remove the current dependency on `stty`
- keep explicit coverage for `write_stdin(chars="")` polling against a live PTY session
- add deterministic coverage that stdin sent through `write_stdin` reaches the child process
- avoid assertions that depend on terminal echo behavior

## Scope

This design covers only daemon exec RPC tests in `crates/remote-exec-daemon/tests/exec_rpc.rs`.

In scope:

- replacing the current `stty`-based PTY truncation test
- adding PTY stdin round-trip coverage
- keeping assertions aligned with current daemon behavior

Out of scope:

- production code changes
- locale normalization changes
- shell resolution changes
- changes to broker tests
- PTY capability probing or server metadata changes

## Current Problem

The existing PTY truncation test currently launches:

- `stty -echo; read line; printf 'one two three four five six'; sleep 30`

That test is trying to prove two things at once:

- `write_stdin` can interact with a live PTY session
- output truncation still works on PTY-backed responses

The `stty -echo` dependency is not part of the daemon contract being tested. It adds operating-system sensitivity without increasing signal. If it fails on a target machine, the failure does not clearly tell us whether the daemon PTY path is broken or whether the local `stty` behavior is simply different.

## Decision Summary

### 1. Split PTY polling/truncation from PTY stdin round-trip

The current combined test should be replaced with two separate tests:

- one PTY polling and truncation test
- one PTY stdin round-trip test

This keeps each test focused on one externally meaningful behavior.

### 2. Use shell builtins only

The replacement commands should rely on shell builtins and simple shell syntax rather than terminal-control utilities.

This keeps the tests closer to the daemon contract and reduces the chance of target-specific failures caused by external tools.

### 3. Do not assert PTY echo behavior

The new round-trip test should not assert the exact full PTY output when stdin is written.

Instead, it should assert that the child process emits a deterministic marker derived from the input text. This preserves useful coverage without depending on whether the PTY or shell echoes the typed line.

## Detailed Test Design

### PTY polling and truncation test

Replace the current `stty`-based truncation test with a `tty=true` command that emits output on its own after a short delay:

- `sleep 0.1; printf 'one two three four five six'; sleep 30`

Test flow:

1. call `/v1/exec/start` with `tty=true`, `yield_time_ms=250`, and `max_output_tokens=3`
2. capture the returned live daemon session id
3. call `/v1/exec/write` with:
   - `chars = ""`
   - `yield_time_ms = 250`
   - `max_output_tokens = 3`
4. assert:
   - the response is still `running`
   - `original_token_count == Some(6)`
   - `output == "one two three"`

This preserves PTY polling coverage through `write_stdin(chars="")` and keeps the truncation assertion exact.

### PTY stdin round-trip test

Add a separate `tty=true` command that blocks on input and then prints a stable marker around the captured line:

- `IFS= read -r line; printf '__RESULT__:%s:__END__' "$line"`

Test flow:

1. call `/v1/exec/start` with `tty=true`
2. capture the returned live daemon session id
3. call `/v1/exec/write` with:
   - `chars = "ping pong\n"`
   - a normal short `yield_time_ms`
4. assert the returned output contains:
   - `__RESULT__:ping pong:__END__`

The test intentionally does not assert the exact full output string, because some systems may echo the typed input before the command prints the marker.

## Rejected Alternatives

### Keep one combined PTY write test

This would preserve a smaller test count, but the assertions would need to be weaker and more tolerant because the test would be mixing truncation and input round-trip in one response.

That makes failures harder to interpret and creates pressure to assert around PTY echo behavior.

### Keep `stty` and soften assertions

This was rejected because it still leaves the suite dependent on terminal-control utility behavior that is outside the daemon contract.

The goal is to improve test portability, not to make a fragile dependency slightly less fragile.

## Risk Assessment

Low risk.

The changes stay entirely inside the test suite and preserve the same daemon-facing behavior coverage:

- live PTY session polling through `write_stdin`
- PTY response truncation
- stdin delivery into a PTY-backed process

The only behavior intentionally dropped is any incidental reliance on terminal echo configuration.

## Verification Plan

Run:

- `cargo test -p remote-exec-daemon --test exec_rpc`

If broader confirmation is wanted after the test edits land, follow with:

- `cargo test -p remote-exec-daemon --test health`
- `cargo test -p remote-exec-daemon --test image_rpc`

## Success Criteria

This design is complete when:

- no daemon exec RPC test depends on `stty`
- PTY polling with `write_stdin(chars="")` still has direct coverage
- PTY stdin round-trip through `write_stdin(chars="...")` has direct coverage
- the round-trip test remains deterministic without asserting terminal echo behavior
