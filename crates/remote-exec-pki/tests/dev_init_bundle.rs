use std::net::{IpAddr, Ipv4Addr};

use remote_exec_pki::{DaemonCertSpec, DevInitSpec, SubjectAltName, build_dev_init_bundle};

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
    assert!(bundle.ca.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.ca.key_pem.contains("BEGIN PRIVATE KEY"));
    assert!(bundle.broker.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(bundle.broker.key_pem.contains("BEGIN PRIVATE KEY"));
    assert!(bundle.daemons.contains_key("builder-a"));
    assert!(
        bundle.daemons["builder-a"]
            .cert_pem
            .contains("BEGIN CERTIFICATE")
    );
    assert!(
        bundle.daemons["builder-a"]
            .key_pem
            .contains("BEGIN PRIVATE KEY")
    );
}
