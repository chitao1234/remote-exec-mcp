# Certificate Management Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `remote-exec-admin certs` with standalone CA/broker/daemon issuance commands and add CA reuse support to `certs dev-init`.

**Architecture:** Refactor `remote-exec-pki` around reusable certificate authority primitives that can either generate a fresh CA or load an existing one from PEM files. Keep `certs dev-init` as the only bundle/manifest-oriented workflow, and build new standalone CLI commands on top of the same PKI issuance and PEM-writing helpers.

**Tech Stack:** Rust 2024, clap, rcgen, serde_json, tempfile, cargo test, cargo fmt, cargo clippy

---

## File Map

- `crates/remote-exec-pki/src/generate.rs`
  - Introduce reusable CA loading/generation primitives and shared broker/daemon issuance functions.
- `crates/remote-exec-pki/src/write.rs`
  - Add standalone PEM-writing helpers for CA, broker, and daemon artifacts while keeping bundle writing intact.
- `crates/remote-exec-pki/src/lib.rs`
  - Re-export the new PKI primitives for the admin crate.
- `crates/remote-exec-pki/tests/ca_reuse.rs`
  - Focused tests for loading an existing CA, rejecting mismatches, and reusing a CA through the bundle path.
- `crates/remote-exec-admin/src/cli.rs`
  - Add `init-ca`, `issue-broker`, `issue-daemon`, and `dev-init` CA reuse flags.
- `crates/remote-exec-admin/src/certs.rs`
  - Command dispatch, CA reuse resolution, and standalone issuance orchestration.
- `crates/remote-exec-admin/tests/certs_issue.rs`
  - Integration coverage for new standalone commands.
- `crates/remote-exec-admin/tests/dev_init.rs`
  - Extend `dev-init` coverage for reused CA workflows and invalid flag combinations.
- `README.md`
  - Document the expanded admin certificate workflow and CA reuse examples.

### Task 1: Refactor `remote-exec-pki` Around Reusable CA Primitives

**Files:**
- Create: `crates/remote-exec-pki/tests/ca_reuse.rs`
- Modify: `crates/remote-exec-pki/src/generate.rs`
- Modify: `crates/remote-exec-pki/src/lib.rs`
- Test/Verify: `cargo test -p remote-exec-pki --test ca_reuse -- --nocapture`

**Testing approach:** `TDD`
Reason: the new CA reuse behavior has a clean PKI seam. The safest way to factor the crate is to prove loading, mismatch rejection, and bundle reuse in focused tests before touching production code.

- [ ] **Step 1: Add failing PKI regression tests for loading and reusing an existing CA**

```rust
// crates/remote-exec-pki/tests/ca_reuse.rs

use remote_exec_pki::{
    DaemonCertSpec, DevInitSpec, build_dev_init_bundle_from_ca, generate_ca, issue_broker_cert,
    issue_daemon_cert, load_ca_from_pem,
};

#[test]
fn load_ca_from_pem_accepts_generated_material_and_reuses_it_in_bundle_output() {
    let ca = generate_ca("remote-exec-ca").expect("generate CA");
    let loaded = load_ca_from_pem(&ca.pem_pair.cert_pem, &ca.pem_pair.key_pem).expect("load CA");
    let spec = DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![DaemonCertSpec::localhost("builder-a")],
    };

    let bundle = build_dev_init_bundle_from_ca(&spec, &loaded).expect("bundle from loaded CA");
    assert_eq!(bundle.ca.cert_pem, ca.pem_pair.cert_pem);
    assert_eq!(bundle.ca.key_pem, ca.pem_pair.key_pem);
    assert!(bundle.broker.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.daemons["builder-a"].cert_pem.contains("BEGIN CERTIFICATE"));
}

#[test]
fn load_ca_from_pem_rejects_mismatched_cert_and_key() {
    let ca_a = generate_ca("remote-exec-ca").expect("first CA");
    let ca_b = generate_ca("remote-exec-ca").expect("second CA");

    let err = load_ca_from_pem(&ca_a.pem_pair.cert_pem, &ca_b.pem_pair.key_pem)
        .expect_err("mismatched CA material must fail");

    assert!(
        err.to_string().contains("match") || err.to_string().contains("CA"),
        "{err:?}"
    );
}

#[test]
fn loaded_ca_can_issue_broker_and_daemon_leaf_certificates() {
    let ca = generate_ca("remote-exec-ca").expect("generate CA");
    let loaded = load_ca_from_pem(&ca.pem_pair.cert_pem, &ca.pem_pair.key_pem).expect("load CA");

    let broker = issue_broker_cert(&loaded, "remote-exec-broker").expect("broker cert");
    let daemon = issue_daemon_cert(&loaded, &DaemonCertSpec::localhost("builder-a"))
        .expect("daemon cert");

    assert!(broker.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(broker.key_pem.contains("BEGIN PRIVATE KEY"));
    assert!(daemon.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(daemon.key_pem.contains("BEGIN PRIVATE KEY"));
}
```

- [ ] **Step 2: Run the focused verification and confirm the new PKI APIs are missing**

Run: `cargo test -p remote-exec-pki --test ca_reuse -- --nocapture`
Expected: FAIL because `generate_ca`, `load_ca_from_pem`, `issue_broker_cert`, `issue_daemon_cert`, and `build_dev_init_bundle_from_ca` are not public crate APIs yet.

- [ ] **Step 3: Implement reusable CA generation/loading and leaf issuance in `remote-exec-pki`**

```rust
// crates/remote-exec-pki/src/generate.rs

use rcgen::{BasicConstraints, Certificate, CertificateParams, DnType, IsCa, KeyPair};

#[derive(Debug, Clone)]
pub struct CertificateAuthority {
    cert: Certificate,
    key: KeyPair,
    pub pem_pair: GeneratedPemPair,
}

pub fn generate_ca(common_name: &str) -> anyhow::Result<CertificateAuthority> {
    let mut params = CertificateParams::new(Vec::new())?;
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok(CertificateAuthority {
        pem_pair: GeneratedPemPair {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
        },
        cert,
        key,
    })
}

pub fn load_ca_from_pem(cert_pem: &str, key_pem: &str) -> anyhow::Result<CertificateAuthority> {
    let params = CertificateParams::from_ca_cert_pem(cert_pem)
        .context("parsing CA certificate PEM")?;
    let key = KeyPair::from_pem(key_pem).context("parsing CA key PEM")?;
    let cert = params
        .self_signed(&key)
        .context("CA certificate and key do not match")?;

    Ok(CertificateAuthority {
        cert,
        key,
        pem_pair: GeneratedPemPair {
            cert_pem: cert_pem.to_string(),
            key_pem: key_pem.to_string(),
        },
    })
}

pub fn issue_broker_cert(
    ca: &CertificateAuthority,
    common_name: &str,
) -> anyhow::Result<GeneratedPemPair> {
    let key = KeyPair::generate()?;
    let params = broker_params(common_name)?;
    let cert = params.signed_by(&key, &ca.cert, &ca.key)?;

    Ok(GeneratedPemPair {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

pub fn issue_daemon_cert(
    ca: &CertificateAuthority,
    daemon: &DaemonCertSpec,
) -> anyhow::Result<GeneratedPemPair> {
    let key = KeyPair::generate()?;
    let params = daemon_params(daemon)?;
    let cert = params.signed_by(&key, &ca.cert, &ca.key)?;

    Ok(GeneratedPemPair {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

pub fn build_dev_init_bundle_from_ca(
    spec: &DevInitSpec,
    ca: &CertificateAuthority,
) -> anyhow::Result<GeneratedDevInitBundle> {
    spec.validate()?;

    let broker = issue_broker_cert(ca, &spec.broker_common_name)?;
    let mut daemons = BTreeMap::new();
    for daemon in &spec.daemon_specs {
        daemons.insert(daemon.target.clone(), issue_daemon_cert(ca, daemon)?);
    }

    Ok(GeneratedDevInitBundle {
        ca: ca.pem_pair.clone(),
        broker,
        daemons,
    })
}

pub fn build_dev_init_bundle(spec: &DevInitSpec) -> anyhow::Result<GeneratedDevInitBundle> {
    let ca = generate_ca(&spec.ca_common_name)?;
    build_dev_init_bundle_from_ca(spec, &ca)
}

// crates/remote-exec-pki/src/lib.rs

pub use generate::{
    CertificateAuthority, GeneratedDevInitBundle, GeneratedPemPair, build_dev_init_bundle,
    build_dev_init_bundle_from_ca, generate_ca, issue_broker_cert, issue_daemon_cert,
    load_ca_from_pem,
};
```

- [ ] **Step 4: Run the post-change verification**

Run:
```bash
cargo test -p remote-exec-pki --test ca_reuse -- --nocapture
cargo test -p remote-exec-pki
```
Expected: both commands PASS, including the new CA reuse tests and the existing bundle generation coverage.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-pki/src/generate.rs \
  crates/remote-exec-pki/src/lib.rs \
  crates/remote-exec-pki/tests/ca_reuse.rs
git commit -m "feat: add reusable CA primitives to remote-exec-pki"
```

### Task 2: Add Standalone PEM Writers And New Admin Certificate Commands

**Files:**
- Modify: `crates/remote-exec-pki/src/write.rs`
- Modify: `crates/remote-exec-pki/src/lib.rs`
- Modify: `crates/remote-exec-admin/src/cli.rs`
- Modify: `crates/remote-exec-admin/src/certs.rs`
- Create: `crates/remote-exec-admin/tests/certs_issue.rs`
- Test/Verify: `cargo test -p remote-exec-admin --test certs_issue -- --nocapture`

**Testing approach:** `TDD`
Reason: the new `init-ca`, `issue-broker`, and `issue-daemon` commands are external CLI behaviors with a clean integration seam. The tests can prove the desired file layout and command surface before implementation.

- [ ] **Step 1: Add failing admin integration tests for `init-ca`, `issue-broker`, and `issue-daemon`**

```rust
// crates/remote-exec-admin/tests/certs_issue.rs

use std::process::Command;

fn admin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
}

#[test]
fn init_ca_writes_only_ca_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("ca");

    let output = admin()
        .args(["certs", "init-ca", "--out-dir"])
        .arg(&out_dir)
        .output()
        .expect("init-ca runs");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(out_dir.join("ca.pem").exists());
    assert!(out_dir.join("ca.key").exists());
    assert!(!out_dir.join("broker.pem").exists());
    assert!(!out_dir.join("certs-manifest.json").exists());
}

#[test]
fn issue_broker_uses_existing_ca_and_writes_only_broker_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let ca_dir = tempdir.path().join("ca");
    let broker_dir = tempdir.path().join("broker");

    let init = admin()
        .args(["certs", "init-ca", "--out-dir"])
        .arg(&ca_dir)
        .output()
        .expect("init-ca runs");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));

    let output = admin()
        .args(["certs", "issue-broker", "--ca-cert-pem"])
        .arg(ca_dir.join("ca.pem"))
        .args(["--ca-key-pem"])
        .arg(ca_dir.join("ca.key"))
        .args(["--out-dir"])
        .arg(&broker_dir)
        .output()
        .expect("issue-broker runs");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(broker_dir.join("broker.pem").exists());
    assert!(broker_dir.join("broker.key").exists());
    assert!(!broker_dir.join("certs-manifest.json").exists());
}

#[test]
fn issue_daemon_writes_target_named_leaf_files() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let ca_dir = tempdir.path().join("ca");
    let daemon_dir = tempdir.path().join("daemon");

    let init = admin()
        .args(["certs", "init-ca", "--out-dir"])
        .arg(&ca_dir)
        .output()
        .expect("init-ca runs");
    assert!(init.status.success(), "{}", String::from_utf8_lossy(&init.stderr));

    let output = admin()
        .args(["certs", "issue-daemon", "--ca-cert-pem"])
        .arg(ca_dir.join("ca.pem"))
        .args(["--ca-key-pem"])
        .arg(ca_dir.join("ca.key"))
        .args(["--out-dir"])
        .arg(&daemon_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("issue-daemon runs");

    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(daemon_dir.join("builder-a.pem").exists());
    assert!(daemon_dir.join("builder-a.key").exists());
    assert!(!daemon_dir.join("certs-manifest.json").exists());
}
```

- [ ] **Step 2: Run the focused verification and confirm the new subcommands are missing**

Run: `cargo test -p remote-exec-admin --test certs_issue -- --nocapture`
Expected: FAIL because `init-ca`, `issue-broker`, and `issue-daemon` are not valid `certs` subcommands yet.

- [ ] **Step 3: Implement standalone writers and CLI command handlers**

```rust
// crates/remote-exec-pki/src/write.rs

pub fn write_ca_pair(
    pair: &GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = KeyPairPaths {
        cert_pem: out_dir.join("ca.pem"),
        key_pem: out_dir.join("ca.key"),
    };
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

pub fn write_broker_pair(
    pair: &GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = KeyPairPaths {
        cert_pem: out_dir.join("broker.pem"),
        key_pem: out_dir.join("broker.key"),
    };
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

pub fn write_daemon_pair(
    target: &str,
    pair: &GeneratedPemPair,
    out_dir: &Path,
    force: bool,
) -> anyhow::Result<KeyPairPaths> {
    let paths = KeyPairPaths {
        cert_pem: out_dir.join(format!("{target}.pem")),
        key_pem: out_dir.join(format!("{target}.key")),
    };
    write_pair(&paths, pair, force)?;
    Ok(paths)
}

fn write_pair(paths: &KeyPairPaths, pair: &GeneratedPemPair, force: bool) -> anyhow::Result<()> {
    validate_output_paths(
        [paths.cert_pem.as_path(), paths.key_pem.as_path()],
        force,
    )?;
    let mut written_paths = Vec::new();
    write_text_file(&paths.cert_pem, &pair.cert_pem, force, 0o644, &mut written_paths)?;
    write_text_file(&paths.key_pem, &pair.key_pem, force, 0o600, &mut written_paths)?;
    Ok(())
}

// crates/remote-exec-admin/src/cli.rs

#[derive(Subcommand, Debug)]
pub enum CertsCommand {
    DevInit(DevInitArgs),
    InitCa(InitCaArgs),
    IssueBroker(IssueBrokerArgs),
    IssueDaemon(IssueDaemonArgs),
}

#[derive(Args, Debug, Clone)]
pub struct InitCaArgs {
    #[arg(long)]
    pub out_dir: PathBuf,
    #[arg(long, default_value = "remote-exec-ca")]
    pub ca_common_name: String,
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Args, Debug, Clone)]
pub struct IssueBrokerArgs {
    #[arg(long)]
    pub out_dir: PathBuf,
    #[arg(long)]
    pub ca_cert_pem: PathBuf,
    #[arg(long)]
    pub ca_key_pem: PathBuf,
    #[arg(long, default_value = "remote-exec-broker")]
    pub broker_common_name: String,
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Args, Debug, Clone)]
pub struct IssueDaemonArgs {
    #[arg(long)]
    pub out_dir: PathBuf,
    #[arg(long)]
    pub ca_cert_pem: PathBuf,
    #[arg(long)]
    pub ca_key_pem: PathBuf,
    #[arg(long)]
    pub target: String,
    #[arg(long = "san")]
    pub sans: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub force: bool,
}

// crates/remote-exec-admin/src/certs.rs

pub fn run(args: CertsArgs) -> anyhow::Result<()> {
    match args.command {
        CertsCommand::DevInit(args) => run_dev_init(args),
        CertsCommand::InitCa(args) => run_init_ca(args),
        CertsCommand::IssueBroker(args) => run_issue_broker(args),
        CertsCommand::IssueDaemon(args) => run_issue_daemon(args),
    }
}

fn run_init_ca(args: InitCaArgs) -> anyhow::Result<()> {
    let ca = remote_exec_pki::generate_ca(&args.ca_common_name)?;
    let paths = remote_exec_pki::write_ca_pair(&ca.pem_pair, &args.out_dir, args.force)?;
    println!("Wrote CA cert: {}", paths.cert_pem.display());
    println!("Wrote CA key: {}", paths.key_pem.display());
    Ok(())
}

fn run_issue_broker(args: IssueBrokerArgs) -> anyhow::Result<()> {
    let ca = load_ca_from_files(&args.ca_cert_pem, &args.ca_key_pem)?;
    let broker = remote_exec_pki::issue_broker_cert(&ca, &args.broker_common_name)?;
    let paths = remote_exec_pki::write_broker_pair(&broker, &args.out_dir, args.force)?;
    println!("Wrote broker cert: {}", paths.cert_pem.display());
    println!("Wrote broker key: {}", paths.key_pem.display());
    Ok(())
}

fn run_issue_daemon(args: IssueDaemonArgs) -> anyhow::Result<()> {
    let ca = load_ca_from_files(&args.ca_cert_pem, &args.ca_key_pem)?;
    let daemon = build_single_daemon_spec(&args)?;
    let pair = remote_exec_pki::issue_daemon_cert(&ca, &daemon)?;
    let paths = remote_exec_pki::write_daemon_pair(&args.target, &pair, &args.out_dir, args.force)?;
    println!("Wrote daemon cert: {}", paths.cert_pem.display());
    println!("Wrote daemon key: {}", paths.key_pem.display());
    Ok(())
}
```

- [ ] **Step 4: Run the post-change verification**

Run:
```bash
cargo test -p remote-exec-admin --test certs_issue -- --nocapture
cargo test -p remote-exec-admin
```
Expected: both commands PASS, proving the new standalone commands write the right files and the existing admin tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-pki/src/write.rs \
  crates/remote-exec-pki/src/lib.rs \
  crates/remote-exec-admin/src/cli.rs \
  crates/remote-exec-admin/src/certs.rs \
  crates/remote-exec-admin/tests/certs_issue.rs
git commit -m "feat: add standalone admin certificate commands"
```

### Task 3: Extend `certs dev-init` With CA Reuse Inputs

**Files:**
- Modify: `crates/remote-exec-admin/src/cli.rs`
- Modify: `crates/remote-exec-admin/src/certs.rs`
- Modify: `crates/remote-exec-admin/tests/dev_init.rs`
- Test/Verify: `cargo test -p remote-exec-admin --test dev_init -- --nocapture`

**Testing approach:** `TDD`
Reason: CA reuse changes user-facing CLI behavior and input validation. The existing `dev_init` integration tests are the right seam to add failing scenarios first.

- [ ] **Step 1: Add failing `dev-init` tests for reused CA success cases and invalid flag combinations**

```rust
// crates/remote-exec-admin/tests/dev_init.rs

#[test]
fn dev_init_reuses_ca_from_previous_bundle_directory() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let reused_dir = tempdir.path().join("reused");

    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&source_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init");
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));

    let second = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&reused_dir)
        .args(["--target", "builder-b", "--reuse-ca-from-dir"])
        .arg(&source_dir)
        .output()
        .expect("reused dev-init");
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));

    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.pem")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.pem")).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.key")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.key")).unwrap()
    );
    assert!(reused_dir.join("broker.pem").exists());
    assert!(reused_dir.join("daemons").join("builder-b.pem").exists());
}

#[test]
fn dev_init_reuses_ca_from_explicit_pem_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let reused_dir = tempdir.path().join("reused");

    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&source_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init");
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));

    let second = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&reused_dir)
        .args(["--target", "builder-b", "--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .args(["--reuse-ca-key-pem"])
        .arg(source_dir.join("ca.key"))
        .output()
        .expect("explicit CA reuse");
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));

    assert_eq!(
        std::fs::read_to_string(source_dir.join("ca.pem")).unwrap(),
        std::fs::read_to_string(reused_dir.join("ca.pem")).unwrap()
    );
}

#[test]
fn dev_init_rejects_partial_or_mixed_reuse_ca_inputs() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let source_dir = tempdir.path().join("source");
    let first = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(&source_dir)
        .args(["--target", "builder-a"])
        .output()
        .expect("initial dev-init");
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));

    let partial = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(tempdir.path().join("partial"))
        .args(["--target", "builder-a", "--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .output()
        .expect("partial reuse");
    assert!(!partial.status.success());
    assert!(String::from_utf8_lossy(&partial.stderr).contains("--reuse-ca-key-pem"));

    let mixed = Command::new(env!("CARGO_BIN_EXE_remote-exec-admin"))
        .args(["certs", "dev-init", "--out-dir"])
        .arg(tempdir.path().join("mixed"))
        .args(["--target", "builder-a", "--reuse-ca-from-dir"])
        .arg(&source_dir)
        .args(["--reuse-ca-cert-pem"])
        .arg(source_dir.join("ca.pem"))
        .args(["--reuse-ca-key-pem"])
        .arg(source_dir.join("ca.key"))
        .output()
        .expect("mixed reuse");
    assert!(!mixed.status.success());
    assert!(String::from_utf8_lossy(&mixed.stderr).contains("--reuse-ca-from-dir"));
}
```

- [ ] **Step 2: Run the focused verification and confirm the reuse inputs are not handled yet**

Run: `cargo test -p remote-exec-admin --test dev_init -- --nocapture`
Expected: FAIL because `dev-init` neither parses nor validates the new reuse flags and still always generates a fresh CA.

- [ ] **Step 3: Implement CA reuse resolution and compose `dev-init` from the new PKI primitives**

```rust
// crates/remote-exec-admin/src/cli.rs

#[derive(Args, Debug, Clone, Default)]
pub struct ReuseCaArgs {
    #[arg(long)]
    pub reuse_ca_cert_pem: Option<PathBuf>,
    #[arg(long)]
    pub reuse_ca_key_pem: Option<PathBuf>,
    #[arg(long)]
    pub reuse_ca_from_dir: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
pub struct DevInitArgs {
    #[arg(long)]
    pub out_dir: PathBuf,
    #[arg(long = "target", required = true)]
    pub targets: Vec<String>,
    #[arg(long = "daemon-san")]
    pub daemon_sans: Vec<String>,
    #[arg(long, default_value = "remote-exec-broker")]
    pub broker_common_name: String,
    #[arg(long, default_value_t = false)]
    pub force: bool,
    #[command(flatten)]
    pub reuse_ca: ReuseCaArgs,
}

// crates/remote-exec-admin/src/certs.rs

fn run_dev_init(args: DevInitArgs) -> anyhow::Result<()> {
    let daemon_specs = build_daemon_specs(&args)?;
    let spec = remote_exec_pki::DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: args.broker_common_name,
        daemon_specs,
    };

    let ca = resolve_dev_init_ca(&args)?;
    let bundle = remote_exec_pki::build_dev_init_bundle_from_ca(&spec, &ca)?;
    let manifest =
        remote_exec_pki::write_dev_init_bundle(&spec, &bundle, &args.out_dir, args.force)?;

    println!("{}", remote_exec_pki::render_config_snippets(&manifest));
    Ok(())
}

fn resolve_dev_init_ca(args: &DevInitArgs) -> anyhow::Result<remote_exec_pki::CertificateAuthority> {
    match (
        args.reuse_ca.reuse_ca_cert_pem.as_ref(),
        args.reuse_ca.reuse_ca_key_pem.as_ref(),
        args.reuse_ca.reuse_ca_from_dir.as_ref(),
    ) {
        (None, None, None) => remote_exec_pki::generate_ca("remote-exec-ca"),
        (Some(cert), Some(key), None) => load_ca_from_files(cert, key),
        (None, None, Some(dir)) => load_ca_from_files(&dir.join("ca.pem"), &dir.join("ca.key")),
        (Some(_), None, _) => anyhow::bail!("--reuse-ca-cert-pem requires --reuse-ca-key-pem"),
        (None, Some(_), _) => anyhow::bail!("--reuse-ca-key-pem requires --reuse-ca-cert-pem"),
        (_, _, Some(_)) => anyhow::bail!(
            "cannot combine --reuse-ca-from-dir with --reuse-ca-cert-pem/--reuse-ca-key-pem"
        ),
    }
}

fn load_ca_from_files(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> anyhow::Result<remote_exec_pki::CertificateAuthority> {
    let cert_pem = std::fs::read_to_string(cert_path)
        .with_context(|| format!("reading {}", cert_path.display()))?;
    let key_pem = std::fs::read_to_string(key_path)
        .with_context(|| format!("reading {}", key_path.display()))?;
    remote_exec_pki::load_ca_from_pem(&cert_pem, &key_pem)
        .with_context(|| format!("loading CA from {} and {}", cert_path.display(), key_path.display()))
}
```

- [ ] **Step 4: Run the post-change verification**

Run:
```bash
cargo test -p remote-exec-admin --test dev_init -- --nocapture
cargo test -p remote-exec-admin
```
Expected: both commands PASS, including reused-CA success paths, invalid flag coverage, and existing overwrite checks.

- [ ] **Step 5: Commit**

```bash
git add crates/remote-exec-admin/src/cli.rs \
  crates/remote-exec-admin/src/certs.rs \
  crates/remote-exec-admin/tests/dev_init.rs
git commit -m "feat: add CA reuse to dev-init"
```

### Task 4: Document The Expanded Workflow And Run The Full Quality Gate

**Files:**
- Modify: `README.md`
- Test/Verify: `cargo test --workspace`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`

**Testing approach:** `existing tests + targeted verification`
Reason: the behavior changes are already covered in Tasks 1-3. This task documents the new commands and finishes with the workspace quality gate required by the repo instructions.

- [ ] **Step 1: Update the README with the new admin certificate commands and reuse flow**

```md
## TLS / CA setup

Preferred bootstrap flow:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs \
  --target builder-a \
  --target builder-b
```

Reuse an existing CA from a prior bundle:

```bash
cargo run -p remote-exec-admin -- certs dev-init \
  --out-dir ./remote-exec-certs-next \
  --target builder-c \
  --reuse-ca-from-dir ./remote-exec-certs
```

Generate only a CA:

```bash
cargo run -p remote-exec-admin -- certs init-ca \
  --out-dir ./remote-exec-ca
```

Issue only a broker client certificate:

```bash
cargo run -p remote-exec-admin -- certs issue-broker \
  --ca-cert-pem ./remote-exec-ca/ca.pem \
  --ca-key-pem ./remote-exec-ca/ca.key \
  --out-dir ./remote-exec-broker-cert
```

Issue one daemon server certificate:

```bash
cargo run -p remote-exec-admin -- certs issue-daemon \
  --ca-cert-pem ./remote-exec-ca/ca.pem \
  --ca-key-pem ./remote-exec-ca/ca.key \
  --out-dir ./remote-exec-daemon-cert \
  --target builder-a \
  --san dns:builder-a.example.com \
  --san ip:10.0.0.12
```
```

- [ ] **Step 2: Re-run the focused admin verification after the README update**

Run:
```bash
cargo test -p remote-exec-admin --test certs_issue -- --nocapture
cargo test -p remote-exec-admin --test dev_init -- --nocapture
```
Expected: PASS.

- [ ] **Step 3: Run the full workspace quality gate**

Run:
```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
```
Expected: all commands PASS cleanly.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: expand admin certificate workflow"
```

## Spec Coverage Check

- Keep `certs dev-init` as the preferred bootstrap command
  - Covered by Task 3 and Task 4.
- Add first-class CA reuse to `dev-init`
  - Covered by Task 3.
- Add lower-level issuance commands
  - Covered by Task 2.
- Reuse one PKI implementation for both `dev-init` and standalone commands
  - Covered by Task 1 and Task 2.
- Preserve current bundle layout and manifest behavior for `dev-init`
  - Covered by Task 3, using existing `write_dev_init_bundle`.
- Keep standalone issuance commands simple and explicit
  - Covered by Task 2.
- Validate CA reuse inputs strictly and fail early on mismatches
  - Covered by Task 1 and Task 3.
