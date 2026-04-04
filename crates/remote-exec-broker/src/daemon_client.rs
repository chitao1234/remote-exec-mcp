use anyhow::Context;
use futures_util::TryStreamExt;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, RpcErrorBody, TRANSFER_CREATE_PARENT_HEADER,
    TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
    TargetInfoResponse, TransferExportRequest, TransferImportRequest, TransferImportResponse,
    TransferSourceType,
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

    pub async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferSourceType, DaemonClientError> {
        let response = self
            .client
            .post(format!("{}{}", self.base_url, "/v1/transfer/export"))
            .header(CONNECTION, "close")
            .json(req)
            .send()
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        if !response.status().is_success() {
            return Err(decode_rpc_error(response).await);
        }

        let source_type = parse_header_enum(response.headers(), TRANSFER_SOURCE_TYPE_HEADER)?;
        let mut file = tokio::fs::File::create(archive_path)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        let mut stream = tokio_util::io::StreamReader::new(
            response.bytes_stream().map_err(std::io::Error::other),
        );
        tokio::io::copy(&mut stream, &mut file)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Ok(source_type)
    }

    pub async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let file = tokio::fs::File::open(archive_path)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
        let response = self
            .client
            .post(format!("{}{}", self.base_url, "/v1/transfer/import"))
            .header(CONNECTION, "close")
            .header(
                TRANSFER_DESTINATION_PATH_HEADER,
                req.destination_path.clone(),
            )
            .header(
                TRANSFER_OVERWRITE_HEADER,
                format_transfer_overwrite(&req.overwrite).to_string(),
            )
            .header(TRANSFER_CREATE_PARENT_HEADER, req.create_parent.to_string())
            .header(
                TRANSFER_SOURCE_TYPE_HEADER,
                format_transfer_source_type(&req.source_type).to_string(),
            )
            .body(body)
            .send()
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        if !response.status().is_success() {
            return Err(decode_rpc_error(response).await);
        }

        response
            .json()
            .await
            .map_err(|err| DaemonClientError::Decode(err.into()))
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

fn format_transfer_source_type(source_type: &TransferSourceType) -> &'static str {
    match source_type {
        TransferSourceType::File => "file",
        TransferSourceType::Directory => "directory",
    }
}

fn format_transfer_overwrite(
    overwrite: &remote_exec_proto::rpc::TransferOverwriteMode,
) -> &'static str {
    match overwrite {
        remote_exec_proto::rpc::TransferOverwriteMode::Fail => "fail",
        remote_exec_proto::rpc::TransferOverwriteMode::Replace => "replace",
    }
}

fn parse_header_enum<T>(
    headers: &reqwest::header::HeaderMap,
    name: &str,
) -> Result<T, DaemonClientError>
where
    T: serde::de::DeserializeOwned,
{
    let raw = headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| DaemonClientError::Decode(anyhow::anyhow!("missing header `{name}`")))?;
    serde_json::from_str::<T>(&format!("\"{raw}\""))
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

async fn decode_rpc_error(response: reqwest::Response) -> DaemonClientError {
    let status = response.status();
    let body = response.text().await.unwrap_or_else(|err| err.to_string());
    if let Ok(error) = serde_json::from_str::<RpcErrorBody>(&body) {
        DaemonClientError::Rpc {
            status,
            code: Some(error.code),
            message: error.message,
        }
    } else {
        DaemonClientError::Rpc {
            status,
            code: None,
            message: body,
        }
    }
}
