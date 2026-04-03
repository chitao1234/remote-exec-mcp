# Certificate Management Extension Design

**Date:** 2026-04-03

## Goal

Extend the admin certificate tooling from a single bootstrap-oriented command into a broader certificate management workflow, while preserving the existing `certs dev-init` convenience path.

The immediate gap to close is CA reuse. Today the admin utility always generates a fresh CA, which makes it unsuitable for incremental certificate issuance or reusing an established trust root.

## Problem

The current `remote-exec-admin certs` surface is too bare bones:

- it only exposes `certs dev-init`
- `dev-init` always generates a fresh CA
- there is no supported path to:
  - generate only a CA
  - issue only a broker certificate
  - issue only a daemon certificate
  - reuse an existing CA from PEM files
  - reuse a CA from a prior `dev-init` output directory

This forces operators back to manual certificate handling as soon as they need anything beyond first-run bootstrap.

## Goals

- Keep `certs dev-init` as the preferred bootstrap command
- Add first-class CA reuse to `dev-init`
- Add lower-level issuance commands so the admin utility is useful beyond initial setup
- Reuse one PKI implementation for both `dev-init` and standalone cert commands
- Preserve the current bundle layout and manifest behavior for `dev-init`
- Keep standalone issuance commands simple and explicit
- Validate CA reuse inputs strictly and fail early on mismatches

## Non-goals

This iteration does not attempt to solve full certificate lifecycle management. It does not include:

- certificate rotation policy
- certificate revocation
- CSR import/signing workflows
- external CA backends
- certificate renewal scheduling
- reuse of previously issued broker or daemon leaf certificates
- automatic mutation of an existing bundle manifest outside `dev-init`

## Current State

Today the relevant structure is:

- `crates/remote-exec-admin/src/cli.rs`
  - defines only `certs dev-init`
- `crates/remote-exec-admin/src/certs.rs`
  - builds one `DevInitSpec`, generates a full bundle, and prints config snippets
- `crates/remote-exec-pki/src/generate.rs`
  - only supports generating a fresh CA and issuing a full dev-init bundle
- `crates/remote-exec-pki/src/write.rs`
  - writes the bundle layout and `certs-manifest.json`

This is already a good foundation. The missing piece is not a new subsystem; it is better factoring and a broader CLI surface over the existing PKI seam.

## Approaches Considered

### 1. Extend only `certs dev-init`

Add `--reuse-ca-*` to `dev-init` and stop there.

Pros:

- smallest surface change
- solves the immediate “cannot reuse CA” problem

Cons:

- still leaves the admin utility too limited for normal cert maintenance
- keeps all issuance logic bundled behind one opinionated workflow

### 2. Replace `dev-init` with low-level commands only

Remove the bootstrap-centric approach and require users to compose CA, broker, and daemon issuance manually.

Pros:

- pure primitives
- low conceptual ambiguity

Cons:

- regresses the best current operator UX
- makes the common bootstrap path worse
- throws away existing bundle/manifest convenience

### 3. Add low-level issuance commands and keep `dev-init` as a composition command

Pros:

- supports both bootstrap and maintenance workflows
- lets `dev-init` reuse the same lower-level PKI operations
- keeps the current convenient path intact
- gives a clean path for future cert-management extensions

Cons:

- larger CLI surface than today
- requires some refactoring in the PKI crate

## Recommendation

Use approach 3.

The admin tool should expose both:

- a bundle-oriented bootstrap command: `certs dev-init`
- low-level issuance commands:
  - `certs init-ca`
  - `certs issue-broker`
  - `certs issue-daemon`

`dev-init` should remain the only command that knows about bundle layout and manifest generation. The low-level commands should write only the PEM artifacts they are responsible for.

## CLI Design

### `certs dev-init`

Purpose:

- generate a ready-to-use certificate bundle
- either with a fresh CA or by reusing an existing CA

Existing inputs remain:

- `--out-dir <dir>`
- `--target <target>` repeated
- `--daemon-san <target>=dns:...|ip:...` repeated
- `--broker-common-name <name>`
- `--force`

New inputs:

- `--reuse-ca-cert-pem <path>`
- `--reuse-ca-key-pem <path>`
- `--reuse-ca-from-dir <dir>`

Behavior:

- if no reuse flags are present, generate a fresh CA
- if `--reuse-ca-cert-pem` is present, `--reuse-ca-key-pem` must also be present
- if `--reuse-ca-from-dir` is present, resolve:
  - `<dir>/ca.pem`
  - `<dir>/ca.key`
- reject mixed use of explicit CA PEM flags and `--reuse-ca-from-dir`
- always issue a fresh broker certificate and fresh daemon certificates for this run
- always write a self-contained bundle to `--out-dir`, including `ca.pem` and `ca.key`

### `certs init-ca`

Purpose:

- generate only a CA certificate and key

Inputs:

- `--out-dir <dir>`
- `--ca-common-name <name>` with the current default
- `--force`

Outputs:

- `ca.pem`
- `ca.key`

### `certs issue-broker`

Purpose:

- issue a broker client certificate from an existing CA

Inputs:

- `--ca-cert-pem <path>`
- `--ca-key-pem <path>`
- `--out-dir <dir>`
- `--broker-common-name <name>`
- `--force`

Outputs:

- `broker.pem`
- `broker.key`

### `certs issue-daemon`

Purpose:

- issue one daemon server certificate from an existing CA

Inputs:

- `--ca-cert-pem <path>`
- `--ca-key-pem <path>`
- `--out-dir <dir>`
- `--target <name>`
- `--san dns:...|ip:...` repeated
- `--force`

Outputs:

- `<target>.pem`
- `<target>.key`

Default SAN behavior:

- if no SANs are provided, reuse the current localhost defaults:
  - `DNS:localhost`
  - `IP:127.0.0.1`

## Output and Layout Rules

### Standalone commands

The standalone commands should not silently adopt bundle-specific layout.

Recommended output behavior:

- `init-ca` writes `ca.pem` and `ca.key` directly under `--out-dir`
- `issue-broker` writes `broker.pem` and `broker.key` directly under `--out-dir`
- `issue-daemon` writes `<target>.pem` and `<target>.key` directly under `--out-dir`

This keeps the commands predictable and avoids hidden coupling to the bundle layout.

### `dev-init`

`dev-init` keeps the current bundle layout exactly:

```text
<out-dir>/
  ca.pem
  ca.key
  broker.pem
  broker.key
  certs-manifest.json
  daemons/
    <target>.pem
    <target>.key
```

When reusing a CA:

- if `--force` is not set, existing destination files still cause failure
- if `--force` is set, the destination `ca.pem` and `ca.key` are overwritten with the reused CA material

This ensures every successful `dev-init` output directory is self-contained and portable.

## PKI Architecture

### Responsibility split

`remote-exec-pki` should own:

- fresh CA generation
- existing CA loading from PEM files
- broker certificate issuance from a CA
- daemon certificate issuance from a CA
- PEM writing
- bundle writing and manifest generation

`remote-exec-admin` should own:

- command-line parsing
- input validation and mutually exclusive flag rules
- mapping CLI arguments into PKI operations
- human-readable output and config snippets

### Internal factoring

The PKI crate should move from a single “build dev-init bundle” path toward composable primitives.

The conceptual API should look like:

- generate a new CA
- load an existing CA from PEM cert/key files
- issue a broker cert from a CA
- issue a daemon cert from a CA
- compose those into `build_dev_init_bundle`

`build_dev_init_bundle` should become “issue bundle from CA material,” not “always generate CA internally.”

That gives `dev-init` one shared engine regardless of whether the CA is fresh or reused.

## Data Flow

### Fresh `dev-init`

1. Parse targets and SANs in `remote-exec-admin`
2. Build daemon specs
3. Generate a fresh CA
4. Issue broker cert from that CA
5. Issue daemon certs from that CA
6. Write the bundle layout and manifest
7. Render config snippets from the manifest

### Reused-CA `dev-init`

1. Parse targets, SANs, and reuse inputs in `remote-exec-admin`
2. Resolve CA source:
   - explicit PEM paths, or
   - `<dir>/ca.pem` and `<dir>/ca.key`
3. Load and validate the CA cert/key pair
4. Issue broker cert from that CA
5. Issue daemon certs from that CA
6. Write the same bundle layout and manifest
7. Render config snippets from the manifest

### Standalone issuance

1. Parse command inputs
2. Load CA if issuing leaves
3. Issue the requested artifact
4. Write only that artifact pair into `--out-dir`
5. Print a concise success summary

## Validation and Error Handling

The new validation rules should be strict and explicit.

### CA reuse validation

- reject partial explicit CA reuse input
- reject mixed explicit and directory-based reuse input
- reject missing `ca.pem` or `ca.key` under `--reuse-ca-from-dir`
- reject unreadable PEM files with path-specific error context
- reject invalid PEM contents with path-specific error context
- reject cert/key mismatches before issuing anything

### Existing validation preserved

- SAN parsing rules remain strict
- unknown targets in `--daemon-san` remain errors
- overwrite still requires `--force`

### Error style

Errors should remain concrete and operator-facing. Prefer messages such as:

- `missing CA key at <path>`
- `invalid PEM certificate at <path>`
- `CA certificate and key do not match`
- `cannot combine --reuse-ca-from-dir with --reuse-ca-cert-pem/--reuse-ca-key-pem`

## Manifest and Snippet Policy

Only `dev-init` should write `certs-manifest.json`.

The standalone issuance commands should not update or reinterpret an existing manifest. That avoids surprising mutations and keeps the primitive commands easy to reason about.

`render_config_snippets` should continue to work from the manifest produced by `dev-init`, and should not be generalized into a standalone command output format in this iteration.

## Testing Strategy

### `remote-exec-pki`

Add focused tests for:

- loading an existing CA from PEM files
- rejecting mismatched CA cert/key pairs
- issuing broker certs from a loaded CA
- issuing daemon certs from a loaded CA
- reusing a CA through the composed `dev-init` path

### `remote-exec-admin`

Add CLI/integration tests for:

- `init-ca` writes only CA files
- `issue-broker` writes broker files from a supplied CA
- `issue-daemon` writes daemon files from a supplied CA
- `dev-init --reuse-ca-cert-pem --reuse-ca-key-pem` succeeds and preserves the CA material
- `dev-init --reuse-ca-from-dir` succeeds
- mixed or partial CA reuse flags fail with clear errors
- existing overwrite behavior still requires `--force`

### Documentation coverage

Update README examples to cover:

- fresh `certs dev-init`
- `certs dev-init --reuse-ca-from-dir`
- `certs init-ca`
- `certs issue-broker`
- `certs issue-daemon`

## Migration and Compatibility

This design is additive:

- existing `certs dev-init` usage continues to work unchanged
- bundle layout and manifest shape remain unchanged
- operators who do not need CA reuse or standalone issuance do not need to change anything

The main compatibility risk is only in argument parsing, so the new CLI flags should be added carefully without changing the current defaults.

## Future Extensions

This design intentionally leaves room for later additions such as:

- issuing multiple daemon certs in one command
- manifest-aware “add target” bundle update workflows
- broker certificate reuse
- daemon certificate reuse
- certificate inspection helpers
- renewal and rotation commands

Those should build on the same PKI primitives introduced here, not add separate parallel code paths.
