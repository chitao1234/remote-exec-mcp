use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct TestCerts {
    pub(crate) ca_cert: PathBuf,
    pub(crate) client_cert: PathBuf,
    pub(crate) client_key: PathBuf,
    pub(crate) daemon_cert: PathBuf,
    pub(crate) daemon_key: PathBuf,
}

pub(crate) fn write_test_certs(dir: &Path) -> TestCerts {
    write_test_certs_for_daemon_spec(dir, remote_exec_pki::DaemonCertSpec::localhost("builder-a"))
}

pub(crate) fn write_test_certs_for_daemon_spec(
    dir: &Path,
    daemon_spec: remote_exec_pki::DaemonCertSpec,
) -> TestCerts {
    let out_dir = dir.join("certs");
    let spec = remote_exec_pki::DevInitSpec {
        ca_common_name: "remote-exec-ca".to_string(),
        broker_common_name: "remote-exec-broker".to_string(),
        daemon_specs: vec![daemon_spec.clone()],
    };
    let bundle = remote_exec_pki::build_dev_init_bundle(&spec).unwrap();
    let manifest = remote_exec_pki::write_dev_init_bundle(&spec, &bundle, &out_dir, true).unwrap();
    let daemon = manifest.daemons.get(&daemon_spec.target).unwrap();

    TestCerts {
        ca_cert: manifest.ca.cert_pem.clone(),
        client_cert: manifest.broker.cert_pem.clone(),
        client_key: manifest.broker.key_pem.clone(),
        daemon_cert: daemon.cert_pem.clone(),
        daemon_key: daemon.key_pem.clone(),
    }
}
