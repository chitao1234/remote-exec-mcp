use anyhow::Context;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};
use reqwest::Identity;
use reqwest::tls::Certificate;

use crate::config::TargetConfig;

#[derive(Clone)]
pub struct DaemonClient {
    client: reqwest::Client,
    base_url: String,
}

impl DaemonClient {
    pub async fn new(config: &TargetConfig) -> anyhow::Result<Self> {
        let ca = Certificate::from_pem(&tokio::fs::read(&config.ca_pem).await?)?;
        let identity = Identity::from_pem(
            &[
                tokio::fs::read(&config.client_cert_pem).await?,
                tokio::fs::read(&config.client_key_pem).await?,
            ]
            .concat(),
        )?;
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .add_root_certificate(ca)
            .identity(identity)
            .build()
            .context("building daemon client")?;

        Ok(Self {
            client,
            base_url: config.base_url.clone(),
        })
    }

    pub async fn target_info(&self) -> anyhow::Result<TargetInfoResponse> {
        self.post("/v1/target-info", &serde_json::json!({})).await
    }

    pub async fn exec_start(&self, req: &ExecStartRequest) -> anyhow::Result<ExecResponse> {
        self.post("/v1/exec/start", req).await
    }

    pub async fn exec_write(&self, req: &ExecWriteRequest) -> anyhow::Result<ExecResponse> {
        self.post("/v1/exec/write", req).await
    }

    pub async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> anyhow::Result<PatchApplyResponse> {
        self.post("/v1/patch/apply", req).await
    }

    pub async fn image_read(&self, req: &ImageReadRequest) -> anyhow::Result<ImageReadResponse> {
        self.post("/v1/image/read", req).await
    }

    async fn post<Req, Resp>(&self, path: &str, body: &Req) -> anyhow::Result<Resp>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        Ok(self
            .client
            .post(format!("{}{}", self.base_url, path))
            .json(body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}
