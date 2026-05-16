use std::path::Path;

pub(super) use remote_exec_test_support::certs::TestCerts;

pub(super) fn write_test_certs(dir: &Path, target: &str) -> TestCerts {
    remote_exec_test_support::certs::write_test_certs_for_daemon_spec(
        dir,
        remote_exec_pki::DaemonCertSpec::localhost(target),
    )
}
