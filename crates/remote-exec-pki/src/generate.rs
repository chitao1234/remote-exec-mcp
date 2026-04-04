use std::collections::BTreeMap;

use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, SanType,
};

use crate::spec::{DaemonCertSpec, DevInitSpec, SubjectAltName};

#[derive(Debug, Clone)]
pub struct GeneratedPemPair {
    pub cert_pem: String,
    pub key_pem: String,
}

#[derive(Debug, Clone)]
pub struct GeneratedDevInitBundle {
    pub ca: GeneratedPemPair,
    pub broker: GeneratedPemPair,
    pub daemons: BTreeMap<String, GeneratedPemPair>,
}

struct GeneratedCa {
    cert: Certificate,
    key: KeyPair,
}

pub fn build_dev_init_bundle(spec: &DevInitSpec) -> anyhow::Result<GeneratedDevInitBundle> {
    spec.validate()?;

    let ca = generate_ca(&spec.ca_common_name)?;
    let broker = issue_broker_cert(&ca, &spec.broker_common_name)?;
    let mut daemons = BTreeMap::new();

    for daemon in &spec.daemon_specs {
        daemons.insert(daemon.target.clone(), issue_daemon_cert(&ca, daemon)?);
    }

    Ok(GeneratedDevInitBundle {
        ca: GeneratedPemPair {
            cert_pem: ca.cert.pem(),
            key_pem: ca.key.serialize_pem(),
        },
        broker,
        daemons,
    })
}

fn generate_ca(common_name: &str) -> anyhow::Result<GeneratedCa> {
    let mut params = CertificateParams::new(Vec::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    Ok(GeneratedCa { cert, key })
}

fn issue_broker_cert(ca: &GeneratedCa, common_name: &str) -> anyhow::Result<GeneratedPemPair> {
    let key = KeyPair::generate()?;
    let params = broker_params(common_name)?;
    let cert = params.signed_by(&key, &ca.cert, &ca.key)?;

    Ok(GeneratedPemPair {
        cert_pem: cert.pem(),
        key_pem: key.serialize_pem(),
    })
}

fn issue_daemon_cert(
    ca: &GeneratedCa,
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

fn broker_params(common_name: &str) -> anyhow::Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    Ok(params)
}

fn daemon_params(daemon: &DaemonCertSpec) -> anyhow::Result<CertificateParams> {
    let dns_names = daemon
        .sans
        .iter()
        .filter_map(|san| match san {
            SubjectAltName::Dns(name) => Some(name.clone()),
            SubjectAltName::Ip(_) => None,
        })
        .collect::<Vec<_>>();

    let mut params = CertificateParams::new(dns_names)?;
    params
        .distinguished_name
        .push(DnType::CommonName, daemon.target.clone());
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];

    for san in &daemon.sans {
        if let SubjectAltName::Ip(addr) = san {
            params.subject_alt_names.push(SanType::IpAddress(*addr));
        }
    }

    Ok(params)
}

#[cfg(test)]
mod tests {
    use rcgen::ExtendedKeyUsagePurpose;

    use super::{broker_params, daemon_params};
    use crate::spec::DaemonCertSpec;

    #[test]
    fn broker_params_use_client_auth_only() {
        let params = broker_params("remote-exec-broker").expect("broker params");
        assert_eq!(
            params.extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ClientAuth]
        );
    }

    #[test]
    fn daemon_params_use_server_auth_and_copy_sans() {
        let params = daemon_params(&DaemonCertSpec::localhost("builder-a")).expect("daemon params");
        assert_eq!(
            params.extended_key_usages,
            vec![ExtendedKeyUsagePurpose::ServerAuth]
        );
        assert!(!params.subject_alt_names.is_empty());
    }
}
