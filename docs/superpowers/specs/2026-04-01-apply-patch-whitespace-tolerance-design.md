# Apply Patch Whitespace Tolerance Design

Status: approved design captured in writing

Date: 2026-04-01

References:

- `docs/local-system-tools.md`
- `crates/remote-exec-daemon/src/patch/parser.rs`
- `crates/remote-exec-daemon/src/patch/mod.rs`
- `crates/remote-exec-daemon/tests/patch_rpc.rs`
- `crates/remote-exec-broker/src/tools/exec_intercept.rs`
- `crates/remote-exec-broker/src/tools/exec.rs`
- `crates/remote-exec-broker/tests/mcp_exec.rs`
- `docs/superpowers/specs/2026-04-01-apply-patch-direct-parity-design.md`
- `docs/superpowers/specs/2026-04-01-apply-patch-exec-interception-design.md`

## Goal

Bring `apply_patch` behavior closer to the updated Codex compatibility notes by accepting horizontal whitespace variation in two places:

- direct patch text marker lines
- explicit `exec_command` interception wrappers for disguised `apply_patch`

This batch is limited to parser tolerance only. It does not expand the patch language or shell grammar.

## Scope

Included:

- tolerate leading and trailing horizontal whitespace on patch control lines
- tolerate extra horizontal whitespace around narrow `exec_command` interception wrapper tokens
- preserve current direct `apply_patch` success and failure output shapes
- preserve current intercepted `exec_command` wrapped output shape
- add focused tests for tolerant direct patch parsing and tolerant interception parsing

Excluded:

- blank-line tolerance
- broader shell parsing
- implicit raw-patch rejection
- warning emission for intercepted `apply_patch`
- freeform/custom-tool `apply_patch`
- patch events
- approval or sandbox behavior
- path restriction hardening

## Compatibility Interpretation

The target is the currently documented Codex-compatible behavior in `docs/local-system-tools.md`:

- the patch runtime parser is slightly more lenient than the declaration grammar and tolerates some leading and trailing whitespace around patch markers
- the interception wrapper parser remains intentionally conservative and should not become a general shell parser

For this repository, that means:

- direct `apply_patch({ target, input, workdir? })` remains the public tool entry point
- the daemon patch parser becomes horizontally whitespace-tolerant on structural lines only
- the broker interception parser becomes horizontally whitespace-tolerant around recognized wrapper tokens only
- everything else stays narrow

## Current Behavior Summary

Today there are two visible tolerance gaps.

First, the daemon patch parser requires exact marker lines:

- `*** Begin Patch`
- `*** End Patch`
- `*** Add File: ...`
- `*** Delete File: ...`
- `*** Update File: ...`
- `*** Move to: ...`
- `@@`
- `@@ context`
- `*** End of File`

That diverges from the updated compatibility notes, which explicitly say the runtime parser tolerates some leading and trailing whitespace around patch markers.

Second, the broker interception parser is still exact-string oriented for shell wrapper structure. It accepts only tightly formatted direct and heredoc forms, which means harmless horizontal whitespace variation around `cd`, `&&`, command name, and `<<` can still prevent interception.

## Decision Summary

### 1. Accept horizontal whitespace on patch control lines

The daemon parser should recognize patch structural lines after trimming leading and trailing spaces or tabs from the whole control line.

Examples that should be treated equivalently:

- `*** Begin Patch`
- `  *** Begin Patch`
- `*** Begin Patch\t`
- ` \t*** Update File: hello.txt\t`

The same rule applies to:

- begin and end markers
- add, delete, update, and move headers
- hunk headers
- end-of-file marker

### 2. Keep payload lines strict

Whitespace tolerance should not change the meaning of payload lines inside hunks.

That means:

- add-file body lines must still begin with `+`
- update hunk lines must still begin with ` `, `-`, or `+`
- file content is preserved exactly after the first payload sigil
- this batch does not normalize or trim file content lines

This keeps the change bounded to structural parsing instead of rewriting patch semantics.

### 3. Accept horizontal whitespace around narrow interception tokens

The broker interception parser should accept extra spaces or tabs around the narrow shell wrapper tokens it already understands:

- between `cd` and its single path argument
- around `&&`
- between command name and quoted patch argument
- around `<<`

Examples that should match:

- ` apply_patch   '...patch...' `
- `applypatch\t\"...patch...\"`
- `cd nested\t&&  apply_patch  <<'PATCH'\n...\nPATCH\n`
- `cd\t nested\t&&\tapplypatch\t<<'PATCH'\n...\nPATCH\n`

This tolerance applies only to the existing accepted forms. It must not broaden interception into general shell parsing.

### 4. Keep the wrapper grammar narrow

The following remain non-matches in this batch:

- `cd a ; apply_patch ...`
- `cd a || apply_patch ...`
- `echo x && apply_patch ...`
- pipelines
- multiple leading or trailing commands
- blank-line-separated wrapper layouts
- raw patch text without explicit `apply_patch`

This preserves the conservative wrapper parser documented in the compatibility notes.

## Rejected Alternatives

### Whole-input normalization before parsing

This would trim or rewrite the raw patch text and command text before the current parsers inspect it.

It was rejected because it risks accidentally changing payload content or masking malformed structure. Token-aware parser tolerance is safer and easier to reason about.

### Broad shell parsing

This would accept many more wrapper layouts and shell syntaxes.

It was rejected because the compatibility notes explicitly describe a narrow, conservative matcher rather than a real shell parser.

### Blank-line tolerance in the same batch

This would accept empty lines before or after markers or around the heredoc wrapper.

It was rejected because the useful compatibility target here is horizontal whitespace variation, not looser line structure. Blank-line tolerance adds ambiguity and increases overmatching risk.

## Code Boundaries

### `crates/remote-exec-daemon/src/patch/parser.rs`

- make structural marker recognition tolerant of leading and trailing spaces or tabs
- preserve current payload-line parsing rules
- keep parse errors explicit when the normalized control line still does not match the patch grammar

### `crates/remote-exec-daemon/src/patch/mod.rs`

- no behavior change beyond using the updated parser
- keep current verification and execution sequencing unchanged

### `crates/remote-exec-daemon/tests/patch_rpc.rs`

- add regression coverage proving tolerant marker parsing works through the public patch RPC surface
- keep existing verification and mutation-order tests unchanged

### `crates/remote-exec-broker/src/tools/exec_intercept.rs`

- replace exact wrapper matching with token-aware horizontal-whitespace parsing
- keep the same accepted direct, alias, heredoc, and `cd <path> && ...` forms
- keep non-matching forms falling back to normal `exec_command`

### `crates/remote-exec-broker/src/tools/exec.rs`

- no flow change beyond benefitting from the updated interception parser
- keep current intercepted success and failure behavior unchanged

### `crates/remote-exec-broker/tests/mcp_exec.rs`

- add or extend broker tests proving tolerant direct and heredoc wrapper forms still intercept
- keep existing wrapped-output expectations unchanged

## Behavior Details

### Direct `apply_patch`

The daemon should accept horizontal-whitespace variation on structural lines only.

Examples:

```text
  *** Begin Patch
*** Add File: hello.txt    
+hello
  *** End Patch
```

```text
*** Begin Patch
	*** Update File: hello.txt
@@
-hello
+hi
*** End Patch	
```

These should behave the same as their tightly formatted equivalents.

### Intercepted `exec_command`

The broker should accept horizontal-whitespace variation while preserving the same narrow grammar.

Examples:

```text
 apply_patch   '*** Begin Patch
*** Add File: hello.txt
+hello
*** End Patch
'
```

```text
cd	 nested  &&  applypatch  <<'PATCH'
*** Begin Patch
*** Add File: hello.txt
+hello
*** End Patch
PATCH
```

These should still route through the patch path and return the same wrapped unified-exec-shaped result as the current explicit forms.

## Error Handling

- malformed structural lines that still do not match after horizontal-whitespace normalization remain parse failures
- non-matching wrapper forms still fall through to normal `exec_command`
- payload-line validation remains unchanged
- this batch does not add any special rejection for implicit raw patch bodies

## Testing Plan

### Daemon parser and RPC coverage

Add coverage for:

- leading indentation on `*** Begin Patch`
- trailing spaces or tabs on `*** End Patch`
- leading or trailing horizontal whitespace on update and move headers
- successful RPC patch application through those tolerant forms

### Broker interception coverage

Add coverage for:

- direct intercepted invocation with extra spaces or tabs before the quoted patch argument
- heredoc interception with extra spaces or tabs around `cd`, `&&`, command name, and `<<`
- tolerant wrapper forms still avoiding daemon `exec_start`
- tolerant wrapper forms still forwarding the expected patch body and effective workdir

### Verification commands

- `cargo test -p remote-exec-daemon --test patch_rpc`
- `cargo test -p remote-exec-broker --test mcp_exec`
- `cargo test --workspace`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`

## Success Criteria

This batch is complete when:

- direct `apply_patch` accepts leading and trailing spaces or tabs on structural patch lines
- `exec_command` interception accepts horizontal-whitespace variation around the existing recognized wrapper tokens
- payload-line semantics remain unchanged
- non-matching broader shell forms still do not intercept
- direct and intercepted output shapes remain unchanged
- blank-line tolerance and raw-patch rejection are still out of scope
