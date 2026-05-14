# Code Quality Audit Security Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. If the user has expressed a strong preference for one of these execution styles, keep using that style unless they explicitly ask to switch. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Plan rule:** This document is a merged design + execution artifact. Any code blocks are illustrative only. Concrete implementation code belongs in the actual code changes, not in this plan.

**Goal:** Resolve the still-live `8.1` PKI hardening issue from section `8.*` of `docs/code-quality-audit.md` and explicitly classify `8.2` as a separate policy question rather than a current security bug.

**Requirements:**
- Fix only the verified live `8.*` issue; do not force speculative changes for stale or overstated claims.
- Preserve current broker/daemon certificate roles and public behavior.
- Keep broker and daemon leaf certificates as end-entity certificates; do not accidentally turn this into a broader certificate-profile redesign.
- Tighten the generated root CA so it can sign end-entity certificates but not intermediate CAs.
- Do not add an explicit certificate lifetime policy in this pass unless a new requirement is approved; `8.2` is not in scope for implementation here.
- Do not edit `docs/code-quality-audit.md`; it remains historical input, not the live contract.

**Architecture:** Treat this as one small PKI hardening task plus a final sweep. The implementation should stay inside `remote-exec-pki`: constrain the generated CA with `BasicConstraints::Constrained(0)` and add regression coverage that proves the root remains a CA while issued broker/daemon certificates remain leaves. The lifetime question stays separate: current code inherits rcgen defaults, and this plan should record that boundary rather than introducing a new validity policy.

**Verification Strategy:** Run focused `remote-exec-pki` tests after the hardening change, then finish with a small sweep that reconfirms the CA constraint and the intentionally unchanged validity-policy behavior.

**Assumptions / Open Questions:**
- `8.1` is real, but the audit’s wording is broader than the actual issue: today’s broker and daemon certificates already default to non-CA leaf certificates through rcgen’s `IsCa::NoCa` default.
- `8.2` is not a live expiry bug on the current dependency set. Local `rcgen 0.14.7` defaults to a very long validity window, so any change there would be about explicit policy, not emergency security remediation.
- If future operator requirements want bounded lifetimes or configurable expiry windows, that should be planned separately so the compatibility and rotation implications are evaluated deliberately.

**Planning-Time Verification Summary:**
- `8.1`: valid and narrowed. `crates/remote-exec-pki/src/generate.rs` still creates the root CA with `IsCa::Ca(BasicConstraints::Unconstrained)`, so the root itself has no path-length limit.
- `8.1` narrowed detail: current broker and daemon leaf certificates are not currently CAs because `broker_params()` and `daemon_params()` build params via `CertificateParams::new(...)`, and local `rcgen 0.14.7` defaults `CertificateParams` to `IsCa::NoCa`.
- `8.2`: partially valid only as an explicitness concern, but invalid as the stated risk. The current dependency version does not default to a 1-year certificate lifetime; local `rcgen 0.14.7` defaults to a long-lived validity window unless overridden.

---

### Task 1: Constrain The Generated Root CA Path Length

**Intent:** Harden the generated root CA so it can sign broker and daemon leaves but cannot authorize a further intermediate CA chain.

**Relevant files/components:**
- Likely modify: `crates/remote-exec-pki/src/generate.rs`
- Likely modify: `crates/remote-exec-pki/tests/ca_reuse.rs`
- Likely inspect: `crates/remote-exec-pki/src/lib.rs`

**Notes / constraints:**
- Cover the verified live portion of `8.1`.
- Change only the root CA basic-constraints policy; do not broaden this into explicit lifetimes, new SAN rules, or key-usage redesign.
- Add regression coverage that checks the generated CA constraint and preserves the current leaf-certificate role assumptions.
- If existing tests do not expose parsed constraints directly, it is acceptable to extend the current PEM parsing approach already used in `remote-exec-pki`.

**Verification:**
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI unit and integration tests still pass after the CA constraint change.
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Expect: the admin workflow that consumes generated PKI material still passes.

- [ ] Confirm the current root-CA and leaf-certificate profile behavior in `remote-exec-pki`
- [ ] Change the generated CA to use a constrained path length of zero
- [ ] Add or extend regression coverage for CA-versus-leaf certificate roles
- [ ] Run focused PKI and admin verification
- [ ] Commit

### Task 2: Final `8.*` Sweep And Explicit Policy Defer

**Intent:** Confirm that the real hardening change landed, and record that certificate validity periods remain an intentional separate policy question rather than an unfinished security fix.

**Relevant files/components:**
- Likely inspect: `docs/code-quality-audit.md`
- Likely inspect: `crates/remote-exec-pki/src/generate.rs`
- Likely inspect: `crates/remote-exec-pki/tests/ca_reuse.rs`

**Notes / constraints:**
- Keep this sweep limited to section `8.*`.
- Reconfirm that `8.2` remains intentionally unchanged in code during this pass.
- If Task 1 adds assertions about current leaf-certificate role behavior, keep them tightly scoped to regression protection rather than expanding them into a new certificate-profile policy layer.

**Verification:**
- Run: `cargo test -p remote-exec-pki`
- Expect: PKI tests remain green.
- Run: `cargo test -p remote-exec-admin --test dev_init`
- Expect: admin bootstrap usage still passes with the constrained CA.

- [ ] Re-run the `8.*` verification queries and confirm the final shape is intentional
- [ ] Re-run the focused PKI and admin verification
- [ ] Summarize which `8.*` items were fixed, narrowed, or intentionally deferred
- [ ] Commit any sweep-only real changes if needed; otherwise do not create an empty commit
