# Built-in Certificate Generation Design

**Date:** 2026-04-01

## Goal

Add a built-in way to generate the certificates needed for broker↔daemon mutual TLS, with a staged design:

- stage 1 delivers a polished dev/bootstrap workflow
- the internal architecture leaves room for future deployment-grade PKI commands

## Problem

Today, users must generate and wire certificates manually. The current README explains one `openssl` flow, but there is no built-in command for:

- creating a local CA
- issuing a broker client certificate
- issuing daemon server certificates
- writing files into a predictable layout
- printing ready-to-use broker and daemon config snippets

The repository already contains duplicated test-only certificate helpers built with `rcgen`, which is a strong signal that the project has a natural reusable PKI seam but has not productized it yet.

## Goals

- Make first-time setup meaningfully easier than the current manual `openssl` flow
- Keep runtime binaries focused on running the broker and daemon
- Reuse one shared PKI implementation for product code and test code
- Preserve the current trust model:
  - TLS authenticates the transport
  - `expected_daemon_name` remains an application-level identity check
- Ship a stage 1 UX that is simple and safe for local clusters and development
- Leave a clean path toward lower-level issuance commands later

## Non-goals

Stage 1 does not attempt to solve full certificate lifecycle management. Specifically, it does not include:

- certificate rotation
- revocation or CRLs
- OCSP
- external CA integration
- CSR signing workflows
- long-term production PKI policy management

## Approaches Considered

### 1. New admin binary plus shared PKI library

Create a dedicated `remote-exec-admin` CLI for operator workflows and a reusable `remote-exec-pki` library for certificate generation and PEM writing.

Pros:

- clean separation between admin UX and runtime binaries
- easiest path from bootstrap UX to richer issuance commands later
- lets tests reuse the same PKI code as user-facing tooling

Cons:

- adds two new crates to the workspace

### 2. Add cert subcommands to broker and daemon binaries

Extend `remote-exec-broker` and/or `remote-exec-daemon` with `certs` subcommands.

Pros:

- fewer binaries

Cons:

- overloads runtime entrypoints that are currently simple
- mixes “run the server” and “administer the system” concerns
- leaves less room for future admin workflows

### 3. Add an `xtask` or shell helper

Pros:

- quickest short-term option

Cons:

- weaker product UX
- less portable and less discoverable
- poor foundation for future PKI commands

## Recommendation

Use approach 1:

- add `remote-exec-admin`
- add `remote-exec-pki`
- ship a single polished stage 1 command first: `remote-exec-admin certs dev-init`

This delivers the immediate UX win without locking the project into a dev-only design.

## User Experience

### Stage 1 command

The first built-in certificate command is:

```text
remote-exec-admin certs dev-init --out-dir ./remote-exec-certs --target builder-a --target builder-b
```

The command is intentionally opinionated. It generates everything required for a local or small test cluster in one run.

### Stage 1 output

The command generates:

- one CA certificate and key
- one broker client certificate and key
- one daemon server certificate and key per target

Suggested output layout:

```text
remote-exec-certs/
  ca.pem
  ca.key
  broker.pem
  broker.key
  certs-manifest.json
  daemons/
    builder-a.pem
    builder-a.key
    builder-b.pem
    builder-b.key
```

### Stage 1 stdout behavior

On success, the command prints:

- the created file paths
- a short explanation of each file’s role
- copy-pasteable broker config snippets
- copy-pasteable daemon config snippets for each target

This keeps the command useful even before the project adds config generation.

## SAN and hostname model

Stage 1 should support both easy local defaults and explicit SAN control.

### Defaults

If the user does not provide daemon SANs explicitly, each daemon certificate gets:

- `DNS:localhost`
- `IP:127.0.0.1`

These defaults optimize for local testing and same-machine bootstrap.

### Explicit SANs

Stage 1 should also allow per-target SAN overrides, using a shape like:

```text
--daemon-san builder-a=dns:builder-a.example.com
--daemon-san builder-a=ip:10.0.0.12
--daemon-san builder-b=dns:builder-b.example.com
```

This is better than a single global `--server-dns` or `--server-ip` flag because real targets often need different hostnames and IPs.

## Architecture

### New crates

- `crates/remote-exec-admin`
- `crates/remote-exec-pki`

### `remote-exec-admin` responsibilities

- `clap`-based command parsing
- user-facing command orchestration
- status output
- config snippet rendering for the generated artifacts

### `remote-exec-pki` responsibilities

- certificate and key generation
- typed certificate specs
- PEM serialization
- overwrite policy
- atomic file writes
- artifact manifest generation

### Runtime binaries remain focused

`remote-exec-broker` and `remote-exec-daemon` should continue to do one thing: load config and run. Certificate generation does not belong in those binaries’ startup interface.

## Proposed file layout

### `crates/remote-exec-admin`

- `crates/remote-exec-admin/src/main.rs`
- `crates/remote-exec-admin/src/cli.rs`
- `crates/remote-exec-admin/src/certs.rs`

### `crates/remote-exec-pki`

- `crates/remote-exec-pki/src/lib.rs`
- `crates/remote-exec-pki/src/spec.rs`
- `crates/remote-exec-pki/src/generate.rs`
- `crates/remote-exec-pki/src/write.rs`
- `crates/remote-exec-pki/src/manifest.rs`

## Internal API shape

The shared PKI library should be organized around reusable building blocks rather than one giant “dev mode” function.

Suggested API shape:

- `generate_ca(spec) -> GeneratedCa`
- `issue_broker_cert(ca, spec) -> GeneratedCert`
- `issue_daemon_cert(ca, spec) -> GeneratedCert`
- `write_bundle(bundle, output_plan) -> WrittenArtifacts`
- `render_config_snippets(manifest) -> String`

Suggested spec types:

- `CaSpec`
- `BrokerCertSpec`
- `DaemonCertSpec`
- `DevInitSpec`

This keeps stage 1 easy to ship while preserving room for later commands such as `init-ca`, `issue-daemon`, or `sign-csr`.

## Validation rules

`remote-exec-admin certs dev-init` should validate all inputs before writing files.

Required validation:

- at least one `--target`
- no duplicate target names
- target names must be safe as filenames and TOML table keys
- every explicit `--daemon-san` must reference a known target
- `--daemon-san` values must use supported prefixes such as `dns:` and `ip:`
- every daemon certificate must end up with at least one SAN
- output directory must be creatable
- writes fail if files already exist, unless `--force` is supplied

## Error handling

The command should behave like a single-shot bootstrap action:

- validate first
- generate in memory second
- write files last

Recommended behavior:

- write keys and certs using atomic temp-file-plus-rename operations where practical
- use restrictive file permissions where the operating system supports them
- fail with actionable messages, for example:
  - duplicate target name
  - unknown target referenced by `--daemon-san`
  - no SANs configured for a target
  - output path already exists; rerun with `--force`

If a write fails after some files have been created, the command should report exactly which paths were written before the failure.

## Artifact manifest

Stage 1 should write a small machine-readable manifest, likely `certs-manifest.json`, containing:

- generation timestamp
- targets
- SANs per target
- output file paths
- validity period metadata

The manifest is useful both for users and for future expansion commands.

## Security and trust-model fit

This feature does not change the trust model.

The existing model stays intact:

- the broker trusts `ca.pem`
- the broker presents its client certificate and key
- each daemon presents its server certificate and key
- each daemon verifies the broker’s client certificate against the CA
- `expected_daemon_name` continues to validate logical target identity above TLS

The built-in generator is there to reduce operator friction, not to replace application-level identity checks.

## Testing strategy

### Unit tests in `remote-exec-pki`

- generated CA PEM parses correctly
- broker certificate PEM parses correctly
- daemon certificate PEM parses correctly
- broker certificate includes client-auth usage
- daemon certificate includes server-auth usage
- SAN handling works for DNS and IP inputs
- validation errors are correct for duplicate targets and malformed SANs

### CLI tests in `remote-exec-admin`

- `certs dev-init` writes the expected files into a temp directory
- `--force` behavior is enforced correctly
- stdout includes usable config snippets

### End-to-end verification

Add an integration test that:

- generates certs using the shared PKI code or the real admin command
- starts a daemon using the generated daemon certificate and key
- connects using the generated broker certificate and key
- verifies successful mutual TLS, ideally through a real broker or daemon request

## Migration plan

The repository currently duplicates `write_test_certs` logic in multiple test support modules. That logic should move into `remote-exec-pki` so the project only has one PKI implementation.

Migration steps:

- extract the duplicated `rcgen` certificate helper into shared product code
- update test support to call the shared helper
- keep only thin test wrappers in test modules

This reduces drift between test-only certificate behavior and the built-in user-facing generator.

## Rollout plan

### Stage 1

- add `remote-exec-admin`
- add `remote-exec-pki`
- implement `remote-exec-admin certs dev-init`
- update the README to recommend the built-in command first
- keep the manual `openssl` flow as a fallback or reference

### Stage 2

Once stage 1 is stable, add lower-level commands such as:

- `remote-exec-admin certs init-ca`
- `remote-exec-admin certs issue-broker`
- `remote-exec-admin certs issue-daemon`
- optionally `remote-exec-admin certs sign-csr`

Stage 2 should build on the same shared library and manifest model, not replace them.

## Summary

The project should add a dedicated admin CLI and a shared PKI library, ship one polished stage 1 bootstrap command, and refactor tests to reuse the same certificate code. That approach improves first-run UX immediately while keeping a clean path toward richer deployment workflows later.
