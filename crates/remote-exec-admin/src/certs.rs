use std::{collections::BTreeMap, fs, net::IpAddr, path::Path};

use anyhow::{Context, ensure};

use crate::cli::{
    CertsArgs, CertsCommand, DevInitArgs, InitCaArgs, IssueBrokerArgs, IssueDaemonArgs,
};

pub fn run(args: CertsArgs) -> anyhow::Result<()> {
    match args.command {
        CertsCommand::DevInit(args) => run_dev_init(args),
        CertsCommand::InitCa(args) => run_init_ca(args),
        CertsCommand::IssueBroker(args) => run_issue_broker(args),
        CertsCommand::IssueDaemon(args) => run_issue_daemon(args),
    }
}

fn run_dev_init(args: DevInitArgs) -> anyhow::Result<()> {
    let daemon_specs = build_daemon_specs(&args)?;
    let ca = resolve_dev_init_ca(&args)?;
    let spec = remote_exec_pki::DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: args.broker_common_name.clone(),
        daemon_specs,
    };

    let bundle = remote_exec_pki::build_dev_init_bundle_from_ca(&spec, &ca)?;
    let manifest =
        remote_exec_pki::write_dev_init_bundle(&spec, &bundle, &args.out_dir, args.force)?;

    println!("{}", crate::bootstrap::render_dev_init_output(&manifest));
    Ok(())
}

fn run_init_ca(args: InitCaArgs) -> anyhow::Result<()> {
    let ca = remote_exec_pki::generate_ca(&args.ca_common_name)?;
    let paths = remote_exec_pki::write_ca_pair(ca.pem_pair(), &args.out_dir, args.force)?;

    println!("Wrote CA cert: {}", paths.cert_pem.display());
    println!("Wrote CA key: {}", paths.key_pem.display());
    Ok(())
}

fn run_issue_broker(args: IssueBrokerArgs) -> anyhow::Result<()> {
    let ca = load_ca_from_files(&args.ca_cert_pem, &args.ca_key_pem)?;
    let broker = ca.issue_broker_cert(&args.broker_common_name)?;
    let paths = remote_exec_pki::write_broker_pair(&broker, &args.out_dir, args.force)?;

    println!("Wrote broker cert: {}", paths.cert_pem.display());
    println!("Wrote broker key: {}", paths.key_pem.display());
    Ok(())
}

fn run_issue_daemon(args: IssueDaemonArgs) -> anyhow::Result<()> {
    let ca = load_ca_from_files(&args.ca_cert_pem, &args.ca_key_pem)?;
    let daemon = build_single_daemon_spec(&args)?;
    let pair = ca.issue_daemon_cert(&daemon)?;
    let paths = remote_exec_pki::write_daemon_pair(&args.target, &pair, &args.out_dir, args.force)?;

    println!("Wrote daemon cert: {}", paths.cert_pem.display());
    println!("Wrote daemon key: {}", paths.key_pem.display());
    Ok(())
}

fn build_daemon_specs(args: &DevInitArgs) -> anyhow::Result<Vec<remote_exec_pki::DaemonCertSpec>> {
    let mut sans_by_target = BTreeMap::<String, Vec<remote_exec_pki::SubjectAltName>>::new();

    for entry in &args.daemon_sans {
        let (target, value) = entry.split_once('=').with_context(|| {
            format!("invalid --san `{entry}`; expected target=dns:... or target=ip:...")
        })?;
        ensure!(
            args.targets.iter().any(|known| known == target),
            "unknown target `{target}` in --san"
        );
        sans_by_target
            .entry(target.to_string())
            .or_default()
            .push(parse_subject_alt_name(value)?);
    }

    let mut daemon_specs = Vec::new();
    for target in &args.targets {
        let sans = sans_by_target.remove(target).unwrap_or_default();
        daemon_specs.push(if sans.is_empty() {
            remote_exec_pki::DaemonCertSpec::localhost(target)
        } else {
            remote_exec_pki::DaemonCertSpec {
                target: target.clone(),
                sans,
            }
        });
    }

    ensure!(
        !daemon_specs.is_empty(),
        "at least one --target is required"
    );
    Ok(daemon_specs)
}

fn parse_subject_alt_name(value: &str) -> anyhow::Result<remote_exec_pki::SubjectAltName> {
    if let Some(host) = value.strip_prefix("dns:") {
        ensure!(!host.trim().is_empty(), "DNS SAN values cannot be empty");
        return Ok(remote_exec_pki::SubjectAltName::Dns(host.to_string()));
    }

    if let Some(ip) = value.strip_prefix("ip:") {
        let ip: IpAddr = ip
            .parse()
            .with_context(|| format!("invalid IP SAN `{ip}`"))?;
        return Ok(remote_exec_pki::SubjectAltName::Ip(ip));
    }

    anyhow::bail!("unsupported SAN `{value}`; expected dns:<hostname> or ip:<address>")
}

fn load_ca_from_files(
    cert_path: &Path,
    key_path: &Path,
) -> anyhow::Result<remote_exec_pki::CertificateAuthority> {
    let cert_pem = fs::read_to_string(cert_path)
        .with_context(|| format!("reading {}", cert_path.display()))?;
    let key_pem =
        fs::read_to_string(key_path).with_context(|| format!("reading {}", key_path.display()))?;
    remote_exec_pki::load_ca_from_pem(&cert_pem, &key_pem).with_context(|| {
        format!(
            "loading CA from {} and {}",
            cert_path.display(),
            key_path.display()
        )
    })
}

fn build_single_daemon_spec(
    args: &IssueDaemonArgs,
) -> anyhow::Result<remote_exec_pki::DaemonCertSpec> {
    let sans = if args.sans.is_empty() {
        remote_exec_pki::DaemonCertSpec::localhost(&args.target).sans
    } else {
        args.sans
            .iter()
            .map(|san| parse_subject_alt_name(san))
            .collect::<anyhow::Result<Vec<_>>>()?
    };

    let daemon = remote_exec_pki::DaemonCertSpec {
        target: args.target.clone(),
        sans,
    };
    daemon.validate()?;
    Ok(daemon)
}

fn resolve_dev_init_ca(
    args: &DevInitArgs,
) -> anyhow::Result<remote_exec_pki::CertificateAuthority> {
    if args.reuse_ca.reuse_ca_from_dir.is_some()
        && (args.reuse_ca.reuse_ca_cert_pem.is_some() || args.reuse_ca.reuse_ca_key_pem.is_some())
    {
        anyhow::bail!(
            "cannot combine --reuse-ca-from-dir with --reuse-ca-cert-pem/--reuse-ca-key-pem"
        );
    }

    if let Some(dir) = args.reuse_ca.reuse_ca_from_dir.as_ref() {
        return load_ca_from_files(
            &dir.join(remote_exec_pki::CA_CERT_FILENAME),
            &dir.join(remote_exec_pki::CA_KEY_FILENAME),
        );
    }

    match (
        args.reuse_ca.reuse_ca_cert_pem.as_ref(),
        args.reuse_ca.reuse_ca_key_pem.as_ref(),
    ) {
        (None, None) => remote_exec_pki::generate_ca("remote-exec-ca"),
        (Some(cert), Some(key)) => load_ca_from_files(cert, key),
        (Some(_), None) => anyhow::bail!("--reuse-ca-cert-pem requires --reuse-ca-key-pem"),
        (None, Some(_)) => anyhow::bail!("--reuse-ca-key-pem requires --reuse-ca-cert-pem"),
    }
}
