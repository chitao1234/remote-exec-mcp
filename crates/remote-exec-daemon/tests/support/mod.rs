use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use base64::Engine;
use image::ImageFormat;
use remote_exec_proto::rpc::RpcErrorBody;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;

pub struct DaemonFixture {
    pub _tempdir: TempDir,
    pub client: reqwest::Client,
    pub addr: SocketAddr,
    #[allow(dead_code)]
    pub workdir: PathBuf,
}

impl DaemonFixture {
    pub fn url(&self, path: &str) -> String {
        format!("https://{}{}", self.addr, path)
    }

    #[allow(dead_code)]
    pub async fn raw_post_json<Req>(&self, path: &str, body: &Req) -> reqwest::Response
    where
        Req: Serialize + ?Sized,
    {
        self.client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    #[allow(dead_code)]
    pub async fn raw_post_bytes(
        &self,
        path: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> reqwest::Response {
        let mut request = self.client.post(self.url(path));
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        request.body(body).send().await.unwrap()
    }

    #[allow(dead_code)]
    pub async fn rpc<Req, Resp>(&self, path: &str, body: &Req) -> Resp
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned,
    {
        self.client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Resp>()
            .await
            .unwrap()
    }

    #[allow(dead_code)]
    pub async fn rpc_error<Req>(&self, path: &str, body: &Req) -> RpcErrorBody
    where
        Req: Serialize + ?Sized,
    {
        let response = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .unwrap();
        assert!(!response.status().is_success());
        response.json::<RpcErrorBody>().await.unwrap()
    }
}

pub async fn spawn_daemon(target: &str) -> DaemonFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config = remote_exec_daemon::config::DaemonConfig {
        target: target.to_string(),
        listen: addr,
        default_workdir: workdir.clone(),
        allow_login_shell: true,
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
        workdir,
    }
}

#[allow(dead_code)]
pub async fn spawn_daemon_with_extra_config(target: &str, extra_config: &str) -> DaemonFixture {
    remote_exec_daemon::install_crypto_provider();

    let tempdir = tempfile::tempdir().unwrap();
    let certs = write_test_certs(tempdir.path(), target);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let config_path = tempdir.path().join("daemon.toml");
    std::fs::write(
        &config_path,
        format!(
            r#"target = "{target}"
listen = "{addr}"
default_workdir = "{}"
{extra_config}

[tls]
cert_pem = "{}"
key_pem = "{}"
ca_pem = "{}"
"#,
            workdir.display(),
            certs.daemon_cert.display(),
            certs.daemon_key.display(),
            certs.ca_cert.display(),
        ),
    )
    .unwrap();
    let config = remote_exec_daemon::config::DaemonConfig::load(&config_path)
        .await
        .unwrap();

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
        workdir,
    }
}

#[allow(dead_code)]
pub async fn write_png(path: &Path, width: u32, height: u32) {
    write_image(path, width, height, ImageFormat::Png).await;
}

#[allow(dead_code)]
pub async fn write_image(path: &Path, width: u32, height: u32, format: ImageFormat) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save_with_format(path, format).unwrap();
}

#[allow(dead_code)]
pub async fn write_invalid_bytes(path: &Path) {
    tokio::fs::write(path, b"not an image").await.unwrap();
}

#[allow(dead_code)]
pub fn decode_data_url(image_url: &str) -> (String, Vec<u8>) {
    let (metadata, data) = image_url.split_once(',').unwrap();
    let mime = metadata
        .strip_prefix("data:")
        .unwrap()
        .strip_suffix(";base64")
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    (mime.to_string(), bytes)
}

struct TestCerts {
    ca_cert: PathBuf,
    client_cert: PathBuf,
    client_key: PathBuf,
    daemon_cert: PathBuf,
    daemon_key: PathBuf,
}

fn write_test_certs(dir: &Path, target: &str) -> TestCerts {
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
