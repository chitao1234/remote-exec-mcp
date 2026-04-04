use std::{collections::BTreeMap, net::IpAddr};

use anyhow::{Context, ensure};

use crate::cli::{CertsArgs, CertsCommand, DevInitArgs};

pub fn run(args: CertsArgs) -> anyhow::Result<()> {
    match args.command {
        CertsCommand::DevInit(args) => run_dev_init(args),
    }
}

fn run_dev_init(args: DevInitArgs) -> anyhow::Result<()> {
    let daemon_specs = build_daemon_specs(&args)?;
    let spec = remote_exec_pki::DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: args.broker_common_name,
        daemon_specs,
    };

    let bundle = remote_exec_pki::build_dev_init_bundle(&spec)?;
    let manifest =
        remote_exec_pki::write_dev_init_bundle(&spec, &bundle, &args.out_dir, args.force)?;

    println!("{}", remote_exec_pki::render_config_snippets(&manifest));
    Ok(())
}

fn build_daemon_specs(args: &DevInitArgs) -> anyhow::Result<Vec<remote_exec_pki::DaemonCertSpec>> {
    let mut sans_by_target = BTreeMap::<String, Vec<remote_exec_pki::SubjectAltName>>::new();

    for entry in &args.daemon_sans {
        let (target, value) = entry.split_once('=').with_context(|| {
            format!("invalid --daemon-san `{entry}`; expected target=dns:... or target=ip:...")
        })?;
        ensure!(
            args.targets.iter().any(|known| known == target),
            "unknown target `{target}` in --daemon-san"
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
