#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use remote_exec_proto::rpc::RpcErrorBody;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;

pub struct DaemonFixture {
    pub _tempdir: TempDir,
    pub client: reqwest::Client,
    pub addr: SocketAddr,
    pub workdir: PathBuf,
}

impl DaemonFixture {
    pub fn url(&self, path: &str) -> String {
        format!("https://{}{}", self.addr, path)
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
