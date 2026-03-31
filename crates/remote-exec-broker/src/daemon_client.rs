use anyhow::Context;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, RpcErrorBody, TargetInfoResponse,
};
use reqwest::Identity;
use reqwest::header::CONNECTION;
use reqwest::tls::Certificate;

use crate::config::TargetConfig;

#[derive(Debug)]
pub enum DaemonClientError {
    Transport(anyhow::Error),
    Rpc {
        status: reqwest::StatusCode,
        code: Option<String>,
        message: String,
    },
    Decode(anyhow::Error),
}

impl DaemonClientError {
    pub fn rpc_code(&self) -> Option<&str> {
        match self {
            Self::Rpc { code, .. } => code.as_deref(),
            _ => None,
        }
    }
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "daemon transport error: {err}"),
            Self::Decode(err) => write!(f, "daemon decode error: {err}"),
            Self::Rpc {
                status,
                code,
                message,
            } => match code {
                Some(code) => write!(f, "{code}: {message} ({status})"),
                None => write!(f, "daemon returned {status}: {message}"),
            },
        }
    }
}

impl std::error::Error for DaemonClientError {}

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

    pub async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        self.post("/v1/target-info", &serde_json::json!({})).await
    }

    pub async fn exec_start(
        &self,
        req: &ExecStartRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.post("/v1/exec/start", req).await
    }

    pub async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.post("/v1/exec/write", req).await
    }

    pub async fn patch_apply(
        &self,
        req: &PatchApplyRequest,
    ) -> Result<PatchApplyResponse, DaemonClientError> {
        self.post("/v1/patch/apply", req).await
    }

    pub async fn image_read(
        &self,
        req: &ImageReadRequest,
    ) -> Result<ImageReadResponse, DaemonClientError> {
        self.post("/v1/image/read", req).await
    }

    async fn post<Req, Resp>(&self, path: &str, body: &Req) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(CONNECTION, "close")
            .json(body)
            .send()
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|err| DaemonClientError::Transport(err.into()))?;
            if let Ok(error) = serde_json::from_str::<RpcErrorBody>(&body) {
                return Err(DaemonClientError::Rpc {
                    status,
                    code: Some(error.code),
                    message: error.message,
                });
            }
            return Err(DaemonClientError::Rpc {
                status,
                code: None,
                message: body,
            });
        }

        response
            .json()
            .await
            .map_err(|err| DaemonClientError::Decode(err.into()))
    }
}
