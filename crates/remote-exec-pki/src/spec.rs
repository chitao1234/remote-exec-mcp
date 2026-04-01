use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr},
};

use anyhow::ensure;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubjectAltName {
    Dns(String),
    Ip(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonCertSpec {
    pub target: String,
    pub sans: Vec<SubjectAltName>,
}

impl DaemonCertSpec {
    pub fn localhost(target: impl Into<String>) -> Self {
        Self {
            target: target.into(),
            sans: vec![
                SubjectAltName::Dns("localhost".to_string()),
                SubjectAltName::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevInitSpec {
    pub ca_common_name: String,
    pub broker_common_name: String,
    pub daemon_specs: Vec<DaemonCertSpec>,
}

impl DevInitSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.daemon_specs.is_empty(),
            "dev-init requires at least one daemon target"
        );

        let mut seen_targets = BTreeSet::new();
        for daemon in &self.daemon_specs {
            ensure!(
                !daemon.target.trim().is_empty(),
                "target names cannot be empty"
            );
            ensure!(
                daemon
                    .target
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
                "target `{}` must be filename-safe and TOML-safe",
                daemon.target
            );
            ensure!(
                seen_targets.insert(daemon.target.clone()),
                "duplicate target `{}`",
                daemon.target
            );
            ensure!(
                !daemon.sans.is_empty(),
                "target `{}` must have at least one SAN",
                daemon.target
            );

            for san in &daemon.sans {
                if let SubjectAltName::Dns(name) = san {
                    ensure!(
                        !name.trim().is_empty(),
                        "target `{}` contains an empty DNS SAN",
                        daemon.target
                    );
                }
            }
        }

        Ok(())
    }
}
