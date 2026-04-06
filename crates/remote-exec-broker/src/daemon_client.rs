use anyhow::Context;
use futures_util::TryStreamExt;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, RpcErrorBody, TRANSFER_COMPRESSION_HEADER,
    TRANSFER_CREATE_PARENT_HEADER, TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER,
    TRANSFER_SOURCE_TYPE_HEADER, TargetInfoResponse, TransferCompression, TransferExportRequest,
    TransferImportRequest, TransferImportResponse, TransferSourceType,
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
    target_name: String,
    base_url: String,
}

impl DaemonClient {
    pub async fn new(
        target_name: impl Into<String>,
        config: &TargetConfig,
    ) -> anyhow::Result<Self> {
        let client = if config.base_url.starts_with("http://") {
            anyhow::ensure!(
                config.allow_insecure_http,
                "http:// targets require allow_insecure_http = true"
            );
            reqwest::Client::builder()
                .build()
                .context("building insecure daemon client")?
        } else {
            let ca_pem = config
                .ca_pem
                .as_ref()
                .context("ca_pem is required for https targets")?;
            let client_cert_pem = config
                .client_cert_pem
                .as_ref()
                .context("client_cert_pem is required for https targets")?;
            let client_key_pem = config
                .client_key_pem
                .as_ref()
                .context("client_key_pem is required for https targets")?;
            let ca = Certificate::from_pem(&tokio::fs::read(ca_pem).await?)?;
            let identity = Identity::from_pem(
                &[
                    tokio::fs::read(client_cert_pem).await?,
                    tokio::fs::read(client_key_pem).await?,
                ]
                .concat(),
            )?;
            reqwest::Client::builder()
                .use_rustls_tls()
                .add_root_certificate(ca)
                .identity(identity)
                .build()
                .context("building daemon client")?
        };

        Ok(Self {
            client,
            target_name: target_name.into(),
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
        let started = std::time::Instant::now();
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            "starting daemon transfer export"
        );
        let response = self
            .client
            .post(format!("{}{}", self.base_url, "/v1/transfer/export"))
            .header(CONNECTION, "close")
            .json(req)
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path = %req.path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "daemon transfer export transport failed"
                );
                DaemonClientError::Transport(err.into())
            })?;
        if !response.status().is_success() {
            tracing::warn!(
                target = %self.target_name,
                base_url = %self.base_url,
                path = %req.path,
                status = response.status().as_u16(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "daemon transfer export returned error status"
            );
            return Err(decode_rpc_error(response).await);
        }

        let source_type = parse_header_enum(response.headers(), TRANSFER_SOURCE_TYPE_HEADER)?;
        let actual_compression =
            parse_optional_header_enum(response.headers(), TRANSFER_COMPRESSION_HEADER)?
                .unwrap_or_default();
        if actual_compression != req.compression {
            return Err(DaemonClientError::Decode(anyhow::anyhow!(
                "target `{}` returned transfer compression `{}` for requested `{}`",
                self.target_name,
                format_transfer_compression(&actual_compression),
                format_transfer_compression(&req.compression)
            )));
        }
        let mut file = tokio::fs::File::create(archive_path)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        let mut stream = tokio_util::io::StreamReader::new(
            response.bytes_stream().map_err(std::io::Error::other),
        );
        tokio::io::copy(&mut stream, &mut file)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer export completed"
        );
        Ok(source_type)
    }

    pub async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        let body = if self.base_url.starts_with("http://") {
            reqwest::Body::from(
                tokio::fs::read(archive_path)
                    .await
                    .map_err(|err| DaemonClientError::Transport(err.into()))?,
            )
        } else {
            let file = tokio::fs::File::open(archive_path)
                .await
                .map_err(|err| DaemonClientError::Transport(err.into()))?;
            reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file))
        };
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
            .header(
                TRANSFER_COMPRESSION_HEADER,
                format_transfer_compression(&req.compression).to_string(),
            )
            .body(body)
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    destination_path = %req.destination_path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "daemon transfer import transport failed"
                );
                DaemonClientError::Transport(err.into())
            })?;
        if !response.status().is_success() {
            tracing::warn!(
                target = %self.target_name,
                base_url = %self.base_url,
                destination_path = %req.destination_path,
                status = response.status().as_u16(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "daemon transfer import returned error status"
            );
            return Err(decode_rpc_error(response).await);
        }

        let summary = response.json().await.map_err(|err| {
            tracing::warn!(
                target = %self.target_name,
                base_url = %self.base_url,
                destination_path = %req.destination_path,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "daemon transfer import decode failed"
            );
            DaemonClientError::Decode(err.into())
        })?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            destination_path = %req.destination_path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer import completed"
        );
        Ok(summary)
    }

    async fn post<Req, Resp>(&self, path: &str, body: &Req) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        let started = std::time::Instant::now();
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            "sending daemon rpc"
        );
        let response = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(CONNECTION, "close")
            .json(body)
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "daemon rpc transport failed"
                );
                DaemonClientError::Transport(err.into())
            })?;
        if !response.status().is_success() {
            tracing::warn!(
                target = %self.target_name,
                base_url = %self.base_url,
                path,
                status = response.status().as_u16(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "daemon rpc returned error status"
            );
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

        let decoded = response.json().await.map_err(|err| {
            tracing::warn!(
                target = %self.target_name,
                base_url = %self.base_url,
                path,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "daemon rpc decode failed"
            );
            DaemonClientError::Decode(err.into())
        })?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon rpc completed"
        );
        Ok(decoded)
    }
}

fn format_transfer_source_type(source_type: &TransferSourceType) -> &'static str {
    match source_type {
        TransferSourceType::File => "file",
        TransferSourceType::Directory => "directory",
        TransferSourceType::Multiple => "multiple",
    }
}

fn format_transfer_compression(compression: &TransferCompression) -> &'static str {
    match compression {
        TransferCompression::None => "none",
        TransferCompression::Zstd => "zstd",
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

fn parse_optional_header_enum<T>(
    headers: &reqwest::header::HeaderMap,
    name: &str,
) -> Result<Option<T>, DaemonClientError>
where
    T: serde::de::DeserializeOwned,
{
    match headers.get(name).and_then(|value| value.to_str().ok()) {
        Some(raw) => serde_json::from_str::<T>(&format!("\"{raw}\""))
            .map(Some)
            .map_err(|err| DaemonClientError::Decode(err.into())),
        None => Ok(None),
    }
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
