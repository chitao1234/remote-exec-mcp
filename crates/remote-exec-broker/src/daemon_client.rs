use futures_util::TryStreamExt;
use remote_exec_proto::rpc::{
    EmptyResponse, ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest,
    ImageReadResponse, PatchApplyRequest, PatchApplyResponse, PortConnectRequest,
    PortConnectResponse, PortConnectionCloseRequest, PortConnectionReadRequest,
    PortConnectionReadResponse, PortConnectionWriteRequest, PortListenAcceptRequest,
    PortListenAcceptResponse, PortListenCloseRequest, PortListenRequest, PortListenResponse,
    PortUdpDatagramReadRequest, PortUdpDatagramReadResponse, PortUdpDatagramWriteRequest,
    RpcErrorBody, TargetInfoResponse, TransferExportMetadata, TransferExportRequest,
    TransferImportMetadata, TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse, TransferSourceType,
};
use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HeaderValue};

use crate::config::{TargetConfig, TargetTransportKind};
use crate::tools::transfer::codec;

#[derive(Debug, Clone)]
pub struct TransferExportResponse {
    pub source_type: TransferSourceType,
}

#[derive(Debug)]
pub struct TransferExportStream {
    pub source_type: TransferSourceType,
    response: reqwest::Response,
}

impl TransferExportStream {
    pub fn into_body(self) -> reqwest::Body {
        reqwest::Body::wrap_stream(self.response.bytes_stream())
    }

    pub fn into_async_read(self) -> impl tokio::io::AsyncRead + Send + Unpin + 'static {
        tokio_util::io::StreamReader::new(
            self.response.bytes_stream().map_err(std::io::Error::other),
        )
    }
}

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
            Self::Transport(err) => write!(f, "daemon transport error: {err:#}"),
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
    authorization: Option<HeaderValue>,
}

impl DaemonClient {
    pub async fn new(
        target_name: impl Into<String>,
        config: &TargetConfig,
    ) -> anyhow::Result<Self> {
        let target_name = target_name.into();
        crate::install_crypto_provider();
        let client = match config.validated_transport(&target_name)? {
            TargetTransportKind::Http => reqwest::Client::builder()
                .build()
                .map_err(anyhow::Error::from)?,
            TargetTransportKind::Https => {
                crate::broker_tls::build_daemon_https_client(config).await?
            }
        };
        let authorization = config
            .http_auth
            .as_ref()
            .map(build_bearer_authorization_header)
            .transpose()?;

        Ok(Self {
            client,
            target_name,
            base_url: config.base_url.clone(),
            authorization,
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

    pub async fn transfer_path_info(
        &self,
        req: &TransferPathInfoRequest,
    ) -> Result<TransferPathInfoResponse, DaemonClientError> {
        self.post("/v1/transfer/path-info", req).await
    }

    pub async fn port_listen(
        &self,
        req: &PortListenRequest,
    ) -> Result<PortListenResponse, DaemonClientError> {
        self.post("/v1/port/listen", req).await
    }

    pub async fn port_listen_accept(
        &self,
        req: &PortListenAcceptRequest,
    ) -> Result<PortListenAcceptResponse, DaemonClientError> {
        self.post("/v1/port/listen/accept", req).await
    }

    pub async fn port_listen_close(
        &self,
        req: &PortListenCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        self.post("/v1/port/listen/close", req).await
    }

    pub async fn port_connect(
        &self,
        req: &PortConnectRequest,
    ) -> Result<PortConnectResponse, DaemonClientError> {
        self.post("/v1/port/connect", req).await
    }

    pub async fn port_connection_read(
        &self,
        req: &PortConnectionReadRequest,
    ) -> Result<PortConnectionReadResponse, DaemonClientError> {
        self.post("/v1/port/connection/read", req).await
    }

    pub async fn port_connection_write(
        &self,
        req: &PortConnectionWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        self.post("/v1/port/connection/write", req).await
    }

    pub async fn port_connection_close(
        &self,
        req: &PortConnectionCloseRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        self.post("/v1/port/connection/close", req).await
    }

    pub async fn port_udp_datagram_read(
        &self,
        req: &PortUdpDatagramReadRequest,
    ) -> Result<PortUdpDatagramReadResponse, DaemonClientError> {
        self.post("/v1/port/udp/read", req).await
    }

    pub async fn port_udp_datagram_write(
        &self,
        req: &PortUdpDatagramWriteRequest,
    ) -> Result<EmptyResponse, DaemonClientError> {
        self.post("/v1/port/udp/write", req).await
    }

    pub async fn transfer_export_to_file(
        &self,
        req: &TransferExportRequest,
        archive_path: &std::path::Path,
    ) -> Result<TransferExportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            "starting daemon transfer export"
        );
        let TransferExportStream {
            source_type,
            response,
        } = self.transfer_export_stream(req).await?;
        self.write_transfer_export_archive(archive_path, response)
            .await?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer export completed"
        );
        Ok(TransferExportResponse { source_type })
    }

    pub async fn transfer_export_stream(
        &self,
        req: &TransferExportRequest,
    ) -> Result<TransferExportStream, DaemonClientError> {
        let started = std::time::Instant::now();
        let response = self.send_transfer_export_request(req, started).await?;
        let metadata = self.transfer_export_metadata(req, response.headers())?;
        Ok(TransferExportStream {
            source_type: metadata.source_type,
            response,
        })
    }

    pub async fn transfer_import_from_file(
        &self,
        archive_path: &std::path::Path,
        req: &TransferImportRequest,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        let (file_len, body) = open_transfer_import_body(archive_path).await?;
        let response = self
            .send_transfer_import_request(req, Some(file_len), body, started)
            .await?;
        let summary = self
            .decode_transfer_import_response(req, started, response)
            .await?;
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            destination_path = %req.destination_path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer import completed"
        );
        Ok(summary)
    }

    pub async fn transfer_import_from_body(
        &self,
        req: &TransferImportRequest,
        body: reqwest::Body,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        let started = std::time::Instant::now();
        let response = self
            .send_transfer_import_request(req, None, body, started)
            .await?;
        self.decode_transfer_import_response(req, started, response)
            .await
    }

    async fn send_transfer_export_request(
        &self,
        req: &TransferExportRequest,
        started: std::time::Instant,
    ) -> Result<reqwest::Response, DaemonClientError> {
        let response = self
            .request("/v1/transfer/export")
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
        self.ensure_transfer_export_success(req, started, response)
            .await
    }

    async fn ensure_transfer_export_success(
        &self,
        req: &TransferExportRequest,
        started: std::time::Instant,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, DaemonClientError> {
        if response.status().is_success() {
            return Ok(response);
        }

        tracing::warn!(
            target = %self.target_name,
            base_url = %self.base_url,
            path = %req.path,
            status = response.status().as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer export returned error status"
        );
        Err(decode_rpc_error(response).await)
    }

    fn transfer_export_metadata(
        &self,
        req: &TransferExportRequest,
        headers: &reqwest::header::HeaderMap,
    ) -> Result<TransferExportMetadata, DaemonClientError> {
        let metadata = codec::parse_export_metadata(headers)?;
        if metadata.compression != req.compression {
            return Err(DaemonClientError::Decode(anyhow::anyhow!(
                "target `{}` returned transfer compression `{}` for requested `{}`",
                self.target_name,
                codec::compression_header_value(&metadata.compression),
                codec::compression_header_value(&req.compression)
            )));
        }

        Ok(metadata)
    }

    async fn write_transfer_export_archive(
        &self,
        archive_path: &std::path::Path,
        response: reqwest::Response,
    ) -> Result<(), DaemonClientError> {
        let mut file = tokio::fs::File::create(archive_path)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        let mut stream = tokio_util::io::StreamReader::new(
            response.bytes_stream().map_err(std::io::Error::other),
        );
        tokio::io::copy(&mut stream, &mut file)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Ok(())
    }

    async fn send_transfer_import_request(
        &self,
        req: &TransferImportRequest,
        file_len: Option<u64>,
        body: reqwest::Body,
        started: std::time::Instant,
    ) -> Result<reqwest::Response, DaemonClientError> {
        let mut request = codec::apply_import_headers(
            self.request("/v1/transfer/import"),
            &TransferImportMetadata::from(req),
        );
        if let Some(file_len) = file_len {
            request = request.header(CONTENT_LENGTH, file_len);
        }
        let response = request.body(body).send().await.map_err(|err| {
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
        self.ensure_transfer_import_success(req, started, response)
            .await
    }

    async fn ensure_transfer_import_success(
        &self,
        req: &TransferImportRequest,
        started: std::time::Instant,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, DaemonClientError> {
        if response.status().is_success() {
            return Ok(response);
        }

        tracing::warn!(
            target = %self.target_name,
            base_url = %self.base_url,
            destination_path = %req.destination_path,
            status = response.status().as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon transfer import returned error status"
        );
        Err(decode_rpc_error(response).await)
    }

    async fn decode_transfer_import_response(
        &self,
        req: &TransferImportRequest,
        started: std::time::Instant,
        response: reqwest::Response,
    ) -> Result<TransferImportResponse, DaemonClientError> {
        response.json().await.map_err(|err| {
            tracing::warn!(
                target = %self.target_name,
                base_url = %self.base_url,
                destination_path = %req.destination_path,
                elapsed_ms = started.elapsed().as_millis() as u64,
                error = %err,
                "daemon transfer import decode failed"
            );
            DaemonClientError::Decode(err.into())
        })
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
            .request(path)
            .json(body)
            .send()
            .await
            .map_err(|err| self.rpc_transport_error(path, started, err))?;
        let response = self.ensure_rpc_success(path, started, response).await?;

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

    async fn ensure_rpc_success(
        &self,
        path: &str,
        started: std::time::Instant,
        response: reqwest::Response,
    ) -> Result<reqwest::Response, DaemonClientError> {
        if response.status().is_success() {
            return Ok(response);
        }

        tracing::warn!(
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            status = response.status().as_u16(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "daemon rpc returned error status"
        );
        let error = decode_rpc_error_strict(response).await?;
        Err(error)
    }

    fn rpc_transport_error(
        &self,
        path: &str,
        started: std::time::Instant,
        err: reqwest::Error,
    ) -> DaemonClientError {
        tracing::warn!(
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            elapsed_ms = started.elapsed().as_millis() as u64,
            error = %err,
            "daemon rpc transport failed"
        );
        DaemonClientError::Transport(err.into())
    }

    fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(CONNECTION, "close");
        if let Some(authorization) = &self.authorization {
            request = request.header(AUTHORIZATION, authorization.clone());
        }
        request
    }
}

fn build_bearer_authorization_header(
    http_auth: &crate::config::HttpAuthConfig,
) -> anyhow::Result<HeaderValue> {
    HeaderValue::from_str(&format!("Bearer {}", http_auth.bearer_token))
        .map_err(anyhow::Error::from)
}

async fn open_transfer_import_body(
    archive_path: &std::path::Path,
) -> Result<(u64, reqwest::Body), DaemonClientError> {
    let file = tokio::fs::File::open(archive_path)
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    let file_len = file
        .metadata()
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?
        .len();
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
    Ok((file_len, body))
}

async fn decode_rpc_error_strict(
    response: reqwest::Response,
) -> Result<DaemonClientError, DaemonClientError> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|err| DaemonClientError::Transport(err.into()))?;
    Ok(decode_rpc_error_body(status, body))
}

async fn decode_rpc_error(response: reqwest::Response) -> DaemonClientError {
    let status = response.status();
    let body = response.text().await.unwrap_or_else(|err| err.to_string());
    decode_rpc_error_body(status, body)
}

fn decode_rpc_error_body(status: reqwest::StatusCode, body: String) -> DaemonClientError {
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
