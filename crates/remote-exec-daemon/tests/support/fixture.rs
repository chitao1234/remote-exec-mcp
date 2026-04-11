use std::net::SocketAddr;
use std::path::PathBuf;
use std::thread::JoinHandle;

use remote_exec_proto::rpc::RpcErrorBody;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tokio::sync::oneshot;

pub struct DaemonFixture {
    pub _tempdir: TempDir,
    pub client: reqwest::Client,
    pub addr: SocketAddr,
    scheme: &'static str,
    #[allow(dead_code, reason = "Shared across daemon integration test crates")]
    pub workdir: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    server_thread: Option<JoinHandle<anyhow::Result<()>>>,
}

#[allow(dead_code, reason = "Shared across daemon integration test crates")]
impl DaemonFixture {
    pub(crate) fn new(
        tempdir: TempDir,
        client: reqwest::Client,
        addr: SocketAddr,
        scheme: &'static str,
        workdir: PathBuf,
        shutdown: oneshot::Sender<()>,
        server_thread: JoinHandle<anyhow::Result<()>>,
    ) -> Self {
        Self {
            _tempdir: tempdir,
            client,
            addr,
            scheme,
            workdir,
            shutdown: Some(shutdown),
            server_thread: Some(server_thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}://{}{}", self.scheme, self.addr, path)
    }

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

impl Drop for DaemonFixture {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(server_thread) = self.server_thread.take() {
            let _ = server_thread.join();
        }
    }
}
