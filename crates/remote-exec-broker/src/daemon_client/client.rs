use remote_exec_proto::request_id::REQUEST_ID_HEADER;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest, FileEditResponse,
    FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse, HealthCheckResponse,
    ImageReadRequest, ImageReadResponse, PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};
use reqwest::header::{AUTHORIZATION, HeaderValue};

use crate::config::{TargetConfig, TargetTimeoutConfig, TargetTransportKind};

use super::{
    DaemonClientError, RpcCallContext, RpcCallKind, RpcErrorDecodePolicy, decode_rpc_error,
};

macro_rules! trace_health_or_warn {
    ($path:expr, $($fields:tt)*) => {
        if $path == "/v1/health" {
            tracing::debug!($($fields)*);
        } else {
            tracing::warn!($($fields)*);
        }
    };
}

#[derive(Clone)]
pub struct DaemonClient {
    pub(super) client: reqwest::Client,
    pub(super) target_name: String,
    pub(super) base_url: String,
    pub(super) authorization: Option<HeaderValue>,
    pub(super) request_timeout: std::time::Duration,
    pub(super) health_probe_timeout: std::time::Duration,
}

#[derive(Clone, Copy)]
enum RpcRetryPolicy {
    None,
    IdempotentMetadata,
}

impl RpcRetryPolicy {
    fn should_retry(self, err: &reqwest::Error, retried: bool) -> bool {
        matches!(self, Self::IdempotentMetadata)
            && !retried
            && is_retryable_idempotent_rpc_transport_error(err)
    }
}

impl DaemonClient {
    pub async fn new(
        target_name: impl Into<String>,
        config: &TargetConfig,
    ) -> anyhow::Result<Self> {
        let target_name = target_name.into();
        crate::install_crypto_provider()?;
        let timeouts = config.timeouts;
        let client = match config.transport_kind() {
            TargetTransportKind::Http => build_http_daemon_client(timeouts)?,
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
            request_timeout: timeouts.request_timeout(),
            health_probe_timeout: timeouts.startup_probe_timeout(),
        })
    }

    pub async fn target_info(&self) -> Result<TargetInfoResponse, DaemonClientError> {
        self.post_idempotent("/v1/target-info", &serde_json::json!({}))
            .await
    }

    pub async fn health(&self) -> Result<HealthCheckResponse, DaemonClientError> {
        self.post_idempotent("/v1/health", &serde_json::json!({}))
            .await
    }

    pub fn health_probe_timeout(&self) -> std::time::Duration {
        self.health_probe_timeout
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

    pub async fn file_read(
        &self,
        req: &FileReadRequest,
    ) -> Result<FileReadResponse, DaemonClientError> {
        self.post("/v1/file/read", req).await
    }

    pub async fn file_write(
        &self,
        req: &FileWriteRequest,
    ) -> Result<FileWriteResponse, DaemonClientError> {
        self.post("/v1/file/write", req).await
    }

    pub async fn file_edit(
        &self,
        req: &FileEditRequest,
    ) -> Result<FileEditResponse, DaemonClientError> {
        self.post("/v1/file/edit", req).await
    }

    pub(super) async fn post<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        self.post_with_retry_policy(path, body, RpcRetryPolicy::None)
            .await
    }

    async fn post_idempotent<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
    ) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        self.post_with_retry_policy(path, body, RpcRetryPolicy::IdempotentMetadata)
            .await
    }

    async fn post_with_retry_policy<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
        retry_policy: RpcRetryPolicy,
    ) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        let started = std::time::Instant::now();
        let context = RpcCallContext::path(
            &self.target_name,
            &self.base_url,
            started,
            RpcCallKind::Rpc,
            path,
        );
        tracing::debug!(
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            "sending daemon rpc"
        );
        let result = tokio::time::timeout(self.request_timeout, async {
            let mut retried = false;
            let response = loop {
                let response = self.request(path).json(body).send().await;
                match response {
                    Ok(response) => {
                        break self
                            .ensure_success(response, RpcErrorDecodePolicy::Strict, |status| {
                                context.log_status_error(status)
                            })
                            .await?;
                    }
                    Err(err) if retry_policy.should_retry(&err, retried) => {
                        retried = true;
                        tracing::debug!(
                            target = %self.target_name,
                            base_url = %self.base_url,
                            path,
                            error = %err,
                            "retrying idempotent daemon rpc after request transport failure"
                        );
                    }
                    Err(err) => {
                        context.log_transport_error(&err);
                        return Err(DaemonClientError::Transport(err.into()));
                    }
                }
            };

            let decoded = self
                .decode_json_response(
                    response,
                    |err| context.log_read_error(err),
                    |err| context.log_decode_error(err),
                )
                .await?;
            context.log_completed();
            Ok(decoded)
        })
        .await;

        match result {
            Ok(result) => result,
            Err(_) => {
                let timeout_ms = self.request_timeout.as_millis() as u64;
                let elapsed_ms = started.elapsed().as_millis() as u64;
                trace_health_or_warn!(
                    path,
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path,
                    elapsed_ms,
                    timeout_ms,
                    "daemon rpc timed out"
                );
                Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "daemon rpc `{path}` timed out after {timeout_ms} ms"
                )))
            }
        }
    }

    pub(super) async fn send_request_with_policy<Send, LogTransport, LogStatus>(
        &self,
        send: Send,
        decode_policy: RpcErrorDecodePolicy,
        log_transport_error: LogTransport,
        log_status_error: LogStatus,
    ) -> Result<reqwest::Response, DaemonClientError>
    where
        Send: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
        LogTransport: FnOnce(&reqwest::Error),
        LogStatus: FnOnce(reqwest::StatusCode),
    {
        let response = send.await.map_err(|err| {
            log_transport_error(&err);
            DaemonClientError::Transport(err.into())
        })?;
        self.ensure_success(response, decode_policy, log_status_error)
            .await
    }

    pub(super) async fn decode_json_response<Resp, LogRead, LogDecode>(
        &self,
        response: reqwest::Response,
        log_read_error: LogRead,
        log_decode_error: LogDecode,
    ) -> Result<Resp, DaemonClientError>
    where
        Resp: serde::de::DeserializeOwned,
        LogRead: FnOnce(&reqwest::Error),
        LogDecode: FnOnce(&serde_json::Error),
    {
        let bytes = response.bytes().await.map_err(|err| {
            log_read_error(&err);
            DaemonClientError::Decode(err.into())
        })?;
        serde_json::from_slice(&bytes).map_err(|err| {
            log_decode_error(&err);
            DaemonClientError::Decode(err.into())
        })
    }

    async fn ensure_success<Log>(
        &self,
        response: reqwest::Response,
        decode_policy: RpcErrorDecodePolicy,
        log_error: Log,
    ) -> Result<reqwest::Response, DaemonClientError>
    where
        Log: FnOnce(reqwest::StatusCode),
    {
        if response.status().is_success() {
            return Ok(response);
        }

        log_error(response.status());
        Err(decode_rpc_error(response, decode_policy).await)
    }

    pub(super) fn rpc_transport_error(
        &self,
        path: &str,
        started: std::time::Instant,
        err: reqwest::Error,
    ) -> DaemonClientError {
        let elapsed_ms = started.elapsed().as_millis() as u64;
        trace_health_or_warn!(
            path,
            target = %self.target_name,
            base_url = %self.base_url,
            path,
            elapsed_ms,
            error = %err,
            "daemon rpc transport failed"
        );
        DaemonClientError::Transport(err.into())
    }

    pub(super) fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let request_id = crate::request_context::current_request_id().unwrap_or_default();
        let mut request = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header(REQUEST_ID_HEADER, request_id.as_str());
        if let Some(authorization) = &self.authorization {
            request = request.header(AUTHORIZATION, authorization.clone());
        }
        request
    }
}

fn build_bearer_authorization_header(
    http_auth: &crate::config::HttpAuthConfig,
) -> anyhow::Result<HeaderValue> {
    HeaderValue::from_str(&http_auth.authorization_header_value()).map_err(anyhow::Error::from)
}

pub(crate) fn apply_daemon_client_timeouts(
    builder: reqwest::ClientBuilder,
    timeouts: TargetTimeoutConfig,
) -> reqwest::ClientBuilder {
    builder
        .connect_timeout(timeouts.connect_timeout())
        .read_timeout(timeouts.read_timeout())
}

fn build_http_daemon_client(timeouts: TargetTimeoutConfig) -> anyhow::Result<reqwest::Client> {
    apply_daemon_client_timeouts(reqwest::Client::builder(), timeouts)
        .build()
        .map_err(anyhow::Error::from)
}

fn is_retryable_idempotent_rpc_transport_error(err: &reqwest::Error) -> bool {
    err.is_request() && !err.is_connect() && !err.is_timeout() && !err.is_builder()
}
