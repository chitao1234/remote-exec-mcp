use std::net::{IpAddr, Ipv4Addr};

use remote_exec_pki::{
    CA_CERT_FILENAME, CA_KEY_FILENAME, DaemonCertSpec, DevInitSpec, SubjectAltName,
    build_dev_init_bundle, write_dev_init_bundle,
};

fn assert_pem_pair(cert_pem: &str, key_pem: &str) {
    assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(key_pem.contains("BEGIN PRIVATE KEY"));
}

#[test]
fn rejects_duplicate_targets() {
    let spec = DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![
            DaemonCertSpec::localhost("builder-a"),
            DaemonCertSpec::localhost("builder-a"),
        ],
    };

    let error = build_dev_init_bundle(&spec).expect_err("duplicate target must fail");
    assert!(error.to_string().contains("duplicate target `builder-a`"));
}

#[test]
fn rejects_empty_common_names() {
    let spec = DevInitSpec {
        ca_common_name: " ".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![DaemonCertSpec::localhost("builder-a")],
    };
    let error = build_dev_init_bundle(&spec).expect_err("empty CA common name must fail");
    assert!(error.to_string().contains("CA common name cannot be empty"));

    let spec = DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "\t".to_string(),
        daemon_specs: vec![DaemonCertSpec::localhost("builder-a")],
    };
    let error = build_dev_init_bundle(&spec).expect_err("empty broker common name must fail");
    assert!(
        error
            .to_string()
            .contains("broker common name cannot be empty")
    );
}

#[test]
fn daemon_cert_spec_validates_without_dev_init_wrapper() {
    let daemon = DaemonCertSpec {
        target: "bad target".to_string(),
        sans: vec![SubjectAltName::Dns("builder-a.example.com".to_string())],
    };
    let error = daemon
        .validate()
        .expect_err("unsafe target names must fail");
    assert!(
        error
            .to_string()
            .contains("must be filename-safe and TOML-safe")
    );

    let daemon = DaemonCertSpec {
        target: "builder-a".to_string(),
        sans: vec![SubjectAltName::Dns(" ".to_string())],
    };
    let error = daemon.validate().expect_err("empty DNS SAN must fail");
    assert!(error.to_string().contains("contains an empty DNS SAN"));
}

#[test]
fn generates_bundle_for_requested_targets() {
    let spec = DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![DaemonCertSpec {
            target: "builder-a".to_string(),
            sans: vec![
                SubjectAltName::Dns("builder-a.example.com".to_string()),
                SubjectAltName::Ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 12))),
            ],
        }],
    };

    let bundle = build_dev_init_bundle(&spec).expect("bundle should generate");
    assert_pem_pair(&bundle.ca.cert_pem, bundle.ca.key_pem.as_str());
    assert_pem_pair(&bundle.broker.cert_pem, bundle.broker.key_pem.as_str());
    assert!(bundle.daemons.contains_key("builder-a"));
    assert_pem_pair(
        &bundle.daemons["builder-a"].cert_pem,
        bundle.daemons["builder-a"].key_pem.as_str(),
    );
}

#[test]
fn writes_dev_init_bundle_with_expected_paths() {
    let spec = DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![DaemonCertSpec::localhost("builder-a")],
    };
    let bundle = build_dev_init_bundle(&spec).expect("bundle should generate");
    let tempdir = tempfile::tempdir().expect("tempdir");
    let out_dir = tempdir.path().join("certs");

    let manifest =
        write_dev_init_bundle(&spec, &bundle, &out_dir, true).expect("bundle should write");

    assert_eq!(manifest.ca.cert_pem, out_dir.join(CA_CERT_FILENAME));
    assert_eq!(manifest.ca.key_pem, out_dir.join(CA_KEY_FILENAME));
    assert_eq!(manifest.broker.cert_pem, out_dir.join("broker.pem"));
    assert_eq!(manifest.broker.key_pem, out_dir.join("broker.key"));
    assert_eq!(
        manifest.daemons["builder-a"].cert_pem,
        out_dir.join("daemons").join("builder-a.pem")
    );
    assert_eq!(
        manifest.daemons["builder-a"].key_pem,
        out_dir.join("daemons").join("builder-a.key")
    );
    assert!(out_dir.join("certs-manifest.json").exists());
}
