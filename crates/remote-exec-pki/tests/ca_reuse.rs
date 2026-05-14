use std::io::Cursor;
use remote_exec_pki::{
    DaemonCertSpec, DevInitSpec, build_dev_init_bundle_from_ca, generate_ca, load_ca_from_pem,
};

fn assert_pem_pair(cert_pem: &str, key_pem: &str) {
    assert!(cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(key_pem.contains("BEGIN PRIVATE KEY"));
}

fn basic_constraints(cert_pem: &str) -> Option<(bool, Option<u32>)> {
    let mut reader = Cursor::new(cert_pem.as_bytes());
    let cert = rustls_pemfile::certs(&mut reader)
        .next()
        .transpose()
        .expect("read certificate PEM")
        .expect("missing certificate PEM block");
    let (_, parsed) =
        x509_parser::parse_x509_certificate(cert.as_ref()).expect("parse certificate DER");
    parsed
        .basic_constraints()
        .expect("read basic constraints")
        .map(|extension| (extension.value.ca, extension.value.path_len_constraint))
}

fn assert_leaf_certificate(cert_pem: &str) {
    let constraints = basic_constraints(cert_pem);
    assert!(
        !matches!(constraints, Some((true, _))),
        "leaf certificate unexpectedly marked as CA: {constraints:?}"
    );
}

#[test]
fn load_ca_from_pem_accepts_generated_material_and_reuses_it_in_bundle_output() {
    let ca = generate_ca("remote-exec-ca").expect("generate CA");
    let loaded = load_ca_from_pem(ca.cert_pem(), ca.key_pem().as_str()).expect("load CA");
    let spec = DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![DaemonCertSpec::localhost("builder-a")],
    };

    let bundle = build_dev_init_bundle_from_ca(&spec, &loaded).expect("bundle from loaded CA");
    assert_eq!(bundle.ca.cert_pem, ca.cert_pem());
    assert_eq!(bundle.ca.key_pem.as_str(), ca.key_pem().as_str());
    assert!(bundle.broker.cert_pem.contains("BEGIN CERTIFICATE"));
    assert!(
        bundle.daemons["builder-a"]
            .cert_pem
            .contains("BEGIN CERTIFICATE")
    );
}

#[test]
fn load_ca_from_pem_rejects_mismatched_cert_and_key() {
    let ca_a = generate_ca("remote-exec-ca").expect("first CA");
    let ca_b = generate_ca("remote-exec-ca").expect("second CA");

    let err = load_ca_from_pem(ca_a.cert_pem(), ca_b.key_pem().as_str())
        .expect_err("mismatched CA material must fail");

    assert!(
        err.to_string().contains("match") || err.to_string().contains("CA"),
        "{err:?}"
    );
}

#[test]
fn loaded_ca_can_issue_broker_and_daemon_leaf_certificates() {
    let ca = generate_ca("remote-exec-ca").expect("generate CA");
    let loaded = load_ca_from_pem(ca.cert_pem(), ca.key_pem().as_str()).expect("load CA");

    let broker = loaded
        .issue_broker_cert("remote-exec-broker")
        .expect("broker cert");
    let daemon = loaded
        .issue_daemon_cert(&DaemonCertSpec::localhost("builder-a"))
        .expect("daemon cert");

    assert_eq!(basic_constraints(ca.cert_pem()), Some((true, Some(0))));
    assert_pem_pair(&broker.cert_pem, broker.key_pem.as_str());
    assert_pem_pair(&daemon.cert_pem, daemon.key_pem.as_str());
    assert_leaf_certificate(&broker.cert_pem);
    assert_leaf_certificate(&daemon.cert_pem);
}
