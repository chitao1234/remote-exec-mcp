This directory contains minimal local crates.io patch copies that keep the workspace building on Rust 1.85.0.

Each patched crate started from the published crates.io source for the same version, then received only the compatibility edits needed for this workspace:

- `darling`, `darling_core`, and `darling_macro`: lowered declared `rust-version` to `1.85.0`
- `process-wrap`: lowered declared `rust-version` and replaced trait-object upcasting with stable downcasting helpers
- `rmcp`: rewrote a small number of `let`-chain expressions that require a newer compiler

Canonical unified diffs live in `patches/`. Each patch is relative to the vendored crate root and should describe the full delta from the pristine crates.io source to the corresponding directory in this tree.

These patches are wired in through the root `Cargo.toml` `[patch.crates-io]` section.
