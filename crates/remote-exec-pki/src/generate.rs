use std::collections::BTreeMap;
use std::fmt;
use std::io::Cursor;

use anyhow::{Context, ensure};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    PublicKeyData, SanType,
};
use zeroize::Zeroizing;

use crate::spec::{DaemonCertSpec, DevInitSpec, SubjectAltName};

#[derive(Debug, Clone)]
pub struct GeneratedPemPair {
    pub cert_pem: String,
    pub key_pem: PrivateKeyPem,
}

pub struct PrivateKeyPem(Zeroizing<String>);

impl PrivateKeyPem {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Clone for PrivateKeyPem {
    fn clone(&self) -> Self {
        Self::new(self.as_str().to_string())
    }
}

impl fmt::Debug for PrivateKeyPem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PrivateKeyPem(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub struct GeneratedDevInitBundle {
    pub ca: GeneratedPemPair,
    pub broker: GeneratedPemPair,
    pub daemons: BTreeMap<String, GeneratedPemPair>,
}

pub struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    pem_pair: GeneratedPemPair,
}

impl CertificateAuthority {
    pub fn cert_pem(&self) -> &str {
        &self.pem_pair.cert_pem
    }

    pub fn key_pem(&self) -> &PrivateKeyPem {
        &self.pem_pair.key_pem
    }

    pub fn pem_pair(&self) -> &GeneratedPemPair {
        &self.pem_pair
    }

    pub fn issue_broker_cert(&self, common_name: &str) -> anyhow::Result<GeneratedPemPair> {
        issue_broker_cert(self, common_name)
    }

    pub fn issue_daemon_cert(&self, daemon: &DaemonCertSpec) -> anyhow::Result<GeneratedPemPair> {
        issue_daemon_cert(self, daemon)
    }
}

impl fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertificateAuthority")
            .field("pem_pair", &self.pem_pair)
            .finish_non_exhaustive()
    }
}

pub fn build_dev_init_bundle(spec: &DevInitSpec) -> anyhow::Result<GeneratedDevInitBundle> {
    let ca = generate_ca(&spec.ca_common_name)?;
    build_dev_init_bundle_from_ca(spec, &ca)
}

pub fn build_dev_init_bundle_from_ca(
    spec: &DevInitSpec,
    ca: &CertificateAuthority,
) -> anyhow::Result<GeneratedDevInitBundle> {
    spec.validate()?;

    let broker = ca.issue_broker_cert(&spec.broker_common_name)?;
    let mut daemons = BTreeMap::new();

    for daemon in &spec.daemon_specs {
        daemons.insert(daemon.target.clone(), ca.issue_daemon_cert(daemon)?);
    }

    Ok(GeneratedDevInitBundle {
        ca: ca.pem_pair.clone(),
        broker,
        daemons,
    })
}

pub fn generate_ca(common_name: &str) -> anyhow::Result<CertificateAuthority> {
    let mut params = CertificateParams::new(Vec::new())?;
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    let key = KeyPair::generate()?;
    let cert = params.self_signed(&key)?;
    let key_pem = key.serialize_pem();
    let issuer: Issuer<'static, KeyPair> = Issuer::new(params, key);
    Ok(CertificateAuthority {
        issuer,
        pem_pair: GeneratedPemPair {
            cert_pem: cert.pem(),
            key_pem: PrivateKeyPem::new(key_pem),
        },
    })
}

pub fn load_ca_from_pem(cert_pem: &str, key_pem: &str) -> anyhow::Result<CertificateAuthority> {
    let key = KeyPair::from_pem(key_pem).context("parsing CA key PEM")?;
    ensure!(
        certificate_public_key_der(cert_pem)? == key.subject_public_key_info(),
        "CA certificate and key do not match"
    );
    let issuer: Issuer<'static, KeyPair> =
        Issuer::from_ca_cert_pem(cert_pem, key).context("parsing CA certificate PEM")?;

    Ok(CertificateAuthority {
        issuer,
        pem_pair: GeneratedPemPair {
            cert_pem: cert_pem.to_string(),
            key_pem: PrivateKeyPem::new(key_pem.to_string()),
        },
    })
}

fn certificate_public_key_der(cert_pem: &str) -> anyhow::Result<Vec<u8>> {
    let mut reader = Cursor::new(cert_pem.as_bytes());
    let cert = rustls_pemfile::certs(&mut reader)
        .next()
        .transpose()
        .context("reading CA certificate PEM")?
        .context("missing CA certificate PEM block")?;
    let (_, parsed) = x509_parser::parse_x509_certificate(cert.as_ref())
        .map_err(|err| anyhow::anyhow!("parsing CA certificate DER: {err}"))?;
    Ok(parsed.public_key().raw.to_vec())
}

fn issue_broker_cert(
    ca: &CertificateAuthority,
    common_name: &str,
) -> anyhow::Result<GeneratedPemPair> {
    let key = KeyPair::generate()?;
    let params = broker_params(common_name)?;
    let cert = params.signed_by(&key, &ca.issuer)?;

    Ok(GeneratedPemPair {
        cert_pem: cert.pem(),
        key_pem: PrivateKeyPem::new(key.serialize_pem()),
    })
}

fn issue_daemon_cert(
    ca: &CertificateAuthority,
    daemon: &DaemonCertSpec,
) -> anyhow::Result<GeneratedPemPair> {
    let key = KeyPair::generate()?;
    let params = daemon_params(daemon)?;
    let cert = params.signed_by(&key, &ca.issuer)?;

    Ok(GeneratedPemPair {
        cert_pem: cert.pem(),
        key_pem: PrivateKeyPem::new(key.serialize_pem()),
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
