use std::path::{Path, PathBuf};

pub(super) struct TestCerts {
    pub(super) ca_cert: PathBuf,
    pub(super) client_cert: PathBuf,
    pub(super) client_key: PathBuf,
    pub(super) daemon_cert: PathBuf,
    pub(super) daemon_key: PathBuf,
}

pub(super) fn write_test_certs(dir: &Path, target: &str) -> TestCerts {
    let out_dir = dir.join("certs");
    let spec = remote_exec_pki::DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![remote_exec_pki::DaemonCertSpec::localhost(target)],
    };
    let bundle = remote_exec_pki::build_dev_init_bundle(&spec).unwrap();
    let manifest = remote_exec_pki::write_dev_init_bundle(&spec, &bundle, &out_dir, true).unwrap();
    let daemon = manifest.daemons.get(target).unwrap();

    TestCerts {
        ca_cert: manifest.ca.cert_pem.clone(),
        client_cert: manifest.broker.cert_pem.clone(),
        client_key: manifest.broker.key_pem.clone(),
        daemon_cert: daemon.cert_pem.clone(),
        daemon_key: daemon.key_pem.clone(),
    }
}
