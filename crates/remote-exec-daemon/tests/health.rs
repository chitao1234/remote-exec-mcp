use remote_exec_proto::rpc::TargetInfoResponse;
use reqwest::StatusCode;

#[tokio::test]
async fn target_info_is_available_over_mutual_tls() {
    let fixture = test_support::spawn_daemon("builder-a").await;

    let health = fixture
        .client
        .post(fixture.url("/v1/health"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let info = fixture
        .client
        .post(fixture.url("/v1/target-info"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap()
        .json::<TargetInfoResponse>()
        .await
        .unwrap();

    assert_eq!(info.target, "builder-a");
    assert_eq!(info.platform, "linux");
    assert!(info.supports_pty);
    assert!(info.supports_image_read);
}

mod test_support {
    use std::net::SocketAddr;
    use std::path::PathBuf;

    use tempfile::TempDir;

    pub struct DaemonFixture {
        pub _tempdir: TempDir,
        pub client: reqwest::Client,
        pub addr: SocketAddr,
    }

    impl DaemonFixture {
        pub fn url(&self, path: &str) -> String {
            format!("https://{}{}", self.addr, path)
        }
    }

    pub async fn spawn_daemon(target: &str) -> DaemonFixture {
        remote_exec_daemon::install_crypto_provider();

        let tempdir = tempfile::tempdir().unwrap();
        let certs = write_test_certs(tempdir.path());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let workdir = tempdir.path().join("workdir");
        std::fs::create_dir_all(&workdir).unwrap();
        let config = remote_exec_daemon::config::DaemonConfig {
            target: target.to_string(),
            listen: addr,
            default_workdir: workdir,
            tls: remote_exec_daemon::config::TlsConfig {
                cert_pem: certs.daemon_cert.clone(),
                key_pem: certs.daemon_key.clone(),
                ca_pem: certs.ca_cert.clone(),
            },
        };

        tokio::spawn(remote_exec_daemon::run(config));

        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(
                reqwest::Certificate::from_pem(&std::fs::read(&certs.ca_cert).unwrap()).unwrap(),
            )
            .identity(
                reqwest::Identity::from_pem(
                    &[
                        std::fs::read(&certs.client_cert).unwrap(),
                        std::fs::read(&certs.client_key).unwrap(),
                    ]
                    .concat(),
                )
                .unwrap(),
            )
            .build()
            .unwrap();

        wait_until_ready(&client, addr).await;
        DaemonFixture {
            _tempdir: tempdir,
            client,
            addr,
        }
    }

    struct TestCerts {
        ca_cert: PathBuf,
        client_cert: PathBuf,
        client_key: PathBuf,
        daemon_cert: PathBuf,
        daemon_key: PathBuf,
    }

    fn write_test_certs(dir: &std::path::Path) -> TestCerts {
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca_cert = rcgen::CertificateParams::new(vec![])
            .unwrap()
            .self_signed(&ca_key)
            .unwrap();

        let mut daemon_params =
            rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        daemon_params
            .subject_alt_names
            .push(rcgen::SanType::IpAddress("127.0.0.1".parse().unwrap()));
        let daemon_key = rcgen::KeyPair::generate().unwrap();
        let daemon_cert = daemon_params
            .signed_by(&daemon_key, &ca_cert, &ca_key)
            .unwrap();

        let client_key = rcgen::KeyPair::generate().unwrap();
        let client_cert = rcgen::CertificateParams::new(vec!["broker".to_string()])
            .unwrap()
            .signed_by(&client_key, &ca_cert, &ca_key)
            .unwrap();

        let ca_cert_path = dir.join("ca.pem");
        let daemon_cert_path = dir.join("daemon.pem");
        let daemon_key_path = dir.join("daemon.key");
        let client_cert_path = dir.join("client.pem");
        let client_key_path = dir.join("client.key");

        std::fs::write(&ca_cert_path, ca_cert.pem()).unwrap();
        std::fs::write(&daemon_cert_path, daemon_cert.pem()).unwrap();
        std::fs::write(&daemon_key_path, daemon_key.serialize_pem()).unwrap();
        std::fs::write(&client_cert_path, client_cert.pem()).unwrap();
        std::fs::write(&client_key_path, client_key.serialize_pem()).unwrap();

        TestCerts {
            ca_cert: ca_cert_path,
            client_cert: client_cert_path,
            client_key: client_key_path,
            daemon_cert: daemon_cert_path,
            daemon_key: daemon_key_path,
        }
    }

    async fn wait_until_ready(client: &reqwest::Client, addr: SocketAddr) {
        for _ in 0..40 {
            if client
                .post(format!("https://{addr}/v1/health"))
                .json(&serde_json::json!({}))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("daemon did not become ready");
    }
}
