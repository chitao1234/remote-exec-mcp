use std::sync::{Arc, RwLock};

use remote_exec_proto::request_id::REQUEST_ID_HEADER;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, FileEditRequest, FileEditResponse,
    FileReadRequest, FileReadResponse, FileWriteRequest, FileWriteResponse, HealthCheckResponse,
    ImageReadRequest, ImageReadResponse, PatchApplyRequest, PatchApplyResponse, TargetInfoResponse,
};
use reqwest::header::{AUTHORIZATION, HeaderValue};

use crate::config::{TargetConfig, TargetTimeoutConfig, TargetTransportKind};
use crate::reverse_transport::ReverseTargetConnection;

use super::{
    DaemonClientError, RpcCallContext, RpcCallKind, RpcErrorDecodePolicy, decode_rpc_error,
};

const EXEC_RPC_TIMEOUT_MARGIN: std::time::Duration = std::time::Duration::from_secs(5);
const CONSECUTIVE_TIMEOUTS_BEFORE_RESET: u8 = 2;

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
    connection: Arc<DaemonConnection>,
    reverse_connection: Option<ReverseTargetConnection>,
    pub(super) target_name: String,
    pub(super) base_url: String,
    pub(super) authorization: Option<HeaderValue>,
    pub(super) request_timeout: std::time::Duration,
    pub(super) health_probe_timeout: std::time::Duration,
}

#[derive(Clone)]
struct DaemonClientFactory {
    target_config: TargetConfig,
    reverse: bool,
}

impl DaemonClientFactory {
    async fn build(&self) -> anyhow::Result<reqwest::Client> {
        let timeouts = self.target_config.timeouts;
        if self.reverse {
            return apply_daemon_client_timeouts(reqwest::Client::builder(), timeouts)
                .pool_max_idle_per_host(0)
                .build()
                .map_err(anyhow::Error::from);
        }
        match self.target_config.transport_kind() {
            TargetTransportKind::Http => build_http_daemon_client(timeouts),
            TargetTransportKind::Https => {
                crate::broker_tls::build_daemon_https_client(&self.target_config).await
            }
        }
    }
}

struct DaemonConnectionState {
    client: reqwest::Client,
    generation: u64,
    consecutive_timeouts: u8,
}

struct DaemonConnection {
    state: RwLock<DaemonConnectionState>,
    recovery_lock: tokio::sync::Mutex<()>,
    factory: DaemonClientFactory,
}

struct DaemonConnectionSnapshot {
    client: reqwest::Client,
    generation: u64,
}

enum TimeoutRecoveryOutcome {
    Deferred { consecutive_timeouts: u8 },
    Superseded,
    Reset,
}

impl DaemonConnection {
    fn snapshot(&self) -> DaemonConnectionSnapshot {
        let state = self.state.read().expect("daemon connection lock poisoned");
        DaemonConnectionSnapshot {
            client: state.client.clone(),
            generation: state.generation,
        }
    }

    fn record_success(&self, generation: u64) {
        let mut state = self.state.write().expect("daemon connection lock poisoned");
        if state.generation == generation {
            state.consecutive_timeouts = 0;
        }
    }

    async fn recover_after_timeout(
        &self,
        expected_generation: Option<u64>,
    ) -> anyhow::Result<TimeoutRecoveryOutcome> {
        let _recovery_guard = self.recovery_lock.lock().await;
        let recovery_generation = {
            let mut state = self.state.write().expect("daemon connection lock poisoned");
            if expected_generation
                .is_some_and(|expected_generation| state.generation != expected_generation)
            {
                return Ok(TimeoutRecoveryOutcome::Superseded);
            }
            state.consecutive_timeouts = state.consecutive_timeouts.saturating_add(1);
            if state.consecutive_timeouts < CONSECUTIVE_TIMEOUTS_BEFORE_RESET {
                return Ok(TimeoutRecoveryOutcome::Deferred {
                    consecutive_timeouts: state.consecutive_timeouts,
                });
            }
            state.generation
        };

        let client = self.factory.build().await?;
        let mut state = self.state.write().expect("daemon connection lock poisoned");
        if state.generation != recovery_generation
            || state.consecutive_timeouts < CONSECUTIVE_TIMEOUTS_BEFORE_RESET
        {
            return Ok(TimeoutRecoveryOutcome::Superseded);
        }
        state.client = client;
        state.generation = state.generation.wrapping_add(1);
        state.consecutive_timeouts = 0;
        Ok(TimeoutRecoveryOutcome::Reset)
    }
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
        reverse_connection: Option<ReverseTargetConnection>,
    ) -> anyhow::Result<Self> {
        let target_name = target_name.into();
        crate::install_crypto_provider()?;
        let timeouts = config.timeouts;
        let factory = DaemonClientFactory {
            target_config: config.clone(),
            reverse: reverse_connection.is_some(),
        };
        let client = factory.build().await?;
        let authorization = config
            .http_auth
            .as_ref()
            .map(build_bearer_authorization_header)
            .transpose()?;

        Ok(Self {
            connection: Arc::new(DaemonConnection {
                state: RwLock::new(DaemonConnectionState {
                    client,
                    generation: 0,
                    consecutive_timeouts: 0,
                }),
                recovery_lock: tokio::sync::Mutex::new(()),
                factory,
            }),
            base_url: reverse_connection.as_ref().map_or_else(
                || config.base_url.clone(),
                |connection| connection.base_url().into(),
            ),
            reverse_connection,
            target_name,
            authorization,
            request_timeout: timeouts.request_timeout(),
            health_probe_timeout: timeouts.startup_probe_timeout(),
        })
    }

    #[cfg(test)]
    pub(super) fn from_test_client(
        client: reqwest::Client,
        base_url: String,
        authorization: Option<HeaderValue>,
        request_timeout: std::time::Duration,
        health_probe_timeout: std::time::Duration,
    ) -> Self {
        let defaults = TargetTimeoutConfig::default();
        let target_config = TargetConfig {
            base_url: base_url.clone(),
            http_auth: None,
            timeouts: TargetTimeoutConfig {
                connect_ms: defaults.connect_ms,
                read_ms: defaults.read_ms,
                request_ms: request_timeout.as_millis() as u64,
                startup_probe_ms: health_probe_timeout.as_millis() as u64,
            },
            ca_pem: None,
            client_cert_pem: None,
            client_key_pem: None,
            allow_insecure_http: true,
            skip_server_name_verification: false,
            pinned_server_cert_pem: None,
            expected_daemon_name: None,
        };
        Self {
            connection: Arc::new(DaemonConnection {
                state: RwLock::new(DaemonConnectionState {
                    client,
                    generation: 0,
                    consecutive_timeouts: 0,
                }),
                recovery_lock: tokio::sync::Mutex::new(()),
                factory: DaemonClientFactory {
                    target_config,
                    reverse: false,
                },
            }),
            reverse_connection: None,
            target_name: remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET.to_string(),
            base_url,
            authorization,
            request_timeout,
            health_probe_timeout,
        }
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
        self.post_with_timeout(
            "/v1/exec/start",
            req,
            self.exec_rpc_timeout(req.yield_time_ms),
        )
        .await
    }

    pub async fn exec_write(
        &self,
        req: &ExecWriteRequest,
    ) -> Result<ExecResponse, DaemonClientError> {
        self.post_with_timeout(
            "/v1/exec/write",
            req,
            self.exec_rpc_timeout(req.yield_time_ms),
        )
        .await
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

    async fn post_with_timeout<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
        timeout: std::time::Duration,
    ) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        self.post_with_retry_policy_and_timeout(path, body, RpcRetryPolicy::None, timeout)
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
        self.post_with_retry_policy_and_timeout(
            path,
            body,
            RpcRetryPolicy::IdempotentMetadata,
            self.request_timeout,
        )
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
        self.post_with_retry_policy_and_timeout(path, body, retry_policy, self.request_timeout)
            .await
    }

    async fn post_with_retry_policy_and_timeout<Req, Resp>(
        &self,
        path: &str,
        body: &Req,
        retry_policy: RpcRetryPolicy,
        timeout: std::time::Duration,
    ) -> Result<Resp, DaemonClientError>
    where
        Req: serde::Serialize + ?Sized,
        Resp: serde::de::DeserializeOwned,
    {
        let started = std::time::Instant::now();
        let connection = self.connection.snapshot();
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
        let result = tokio::time::timeout(timeout, async {
            let mut retried = false;
            let response = loop {
                let response = self
                    .request_with_client(&connection.client, path)
                    .json(body)
                    .send()
                    .await;
                match response {
                    Ok(response) => {
                        if !response.status().is_success() {
                            self.connection.record_success(connection.generation);
                        }
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
                        if err.is_timeout() {
                            let recovery = self
                                .recover_connection_after_timeout(path, Some(connection.generation))
                                .await;
                            return Err(DaemonClientError::Transport(anyhow::anyhow!(
                                "daemon rpc `{path}` timed out; {recovery}"
                            )));
                        }
                        return Err(DaemonClientError::Transport(err.into()));
                    }
                }
            };

            let decoded = self
                .decode_json_response(
                    response,
                    connection.generation,
                    path,
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
                let timeout_ms = timeout.as_millis() as u64;
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
                let recovery = self
                    .recover_connection_after_timeout(path, Some(connection.generation))
                    .await;
                Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "daemon rpc `{path}` timed out after {timeout_ms} ms; {recovery}"
                )))
            }
        }
    }

    fn exec_rpc_timeout(&self, yield_time_ms: Option<u64>) -> std::time::Duration {
        let Some(yield_time_ms) = yield_time_ms else {
            return self.request_timeout;
        };
        let requested =
            std::time::Duration::from_millis(yield_time_ms).saturating_add(EXEC_RPC_TIMEOUT_MARGIN);
        self.request_timeout.max(requested)
    }

    pub(super) async fn send_request_with_policy<Send, LogTransport, LogStatus>(
        &self,
        send: Send,
        connection_generation: u64,
        operation: &str,
        decode_policy: RpcErrorDecodePolicy,
        log_transport_error: LogTransport,
        log_status_error: LogStatus,
    ) -> Result<reqwest::Response, DaemonClientError>
    where
        Send: std::future::Future<Output = Result<reqwest::Response, reqwest::Error>>,
        LogTransport: FnOnce(&reqwest::Error),
        LogStatus: FnOnce(reqwest::StatusCode),
    {
        let response = match send.await {
            Ok(response) => {
                if !response.status().is_success() {
                    self.connection.record_success(connection_generation);
                }
                response
            }
            Err(err) => {
                log_transport_error(&err);
                if err.is_timeout() {
                    let recovery = self
                        .recover_connection_after_timeout(operation, Some(connection_generation))
                        .await;
                    return Err(DaemonClientError::Transport(anyhow::anyhow!(
                        "{operation} timed out; {recovery}"
                    )));
                }
                return Err(DaemonClientError::Transport(err.into()));
            }
        };
        self.ensure_success(response, decode_policy, log_status_error)
            .await
    }

    pub(super) async fn decode_json_response<Resp, LogRead, LogDecode>(
        &self,
        response: reqwest::Response,
        connection_generation: u64,
        operation: &str,
        log_read_error: LogRead,
        log_decode_error: LogDecode,
    ) -> Result<Resp, DaemonClientError>
    where
        Resp: serde::de::DeserializeOwned,
        LogRead: FnOnce(&reqwest::Error),
        LogDecode: FnOnce(&serde_json::Error),
    {
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(err) => {
                log_read_error(&err);
                if err.is_timeout() {
                    let recovery = self
                        .recover_connection_after_timeout(operation, Some(connection_generation))
                        .await;
                    return Err(DaemonClientError::Transport(anyhow::anyhow!(
                        "{operation} timed out while reading the response body; {recovery}"
                    )));
                }
                return Err(DaemonClientError::Decode(err.into()));
            }
        };
        self.connection.record_success(connection_generation);
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

    pub(crate) async fn recover_connection_after_timeout(
        &self,
        operation: &str,
        expected_generation: Option<u64>,
    ) -> String {
        match self
            .connection
            .recover_after_timeout(expected_generation)
            .await
        {
            Ok(TimeoutRecoveryOutcome::Deferred {
                consecutive_timeouts,
            }) => {
                tracing::debug!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    operation,
                    consecutive_timeouts,
                    "retained daemon connections after isolated timeout"
                );
                format!(
                    "connection was retained after {consecutive_timeouts} consecutive timeout; request was not replayed"
                )
            }
            Ok(TimeoutRecoveryOutcome::Superseded) => {
                "connection recovery was already completed or no longer needed; request was not replayed"
                    .to_string()
            }
            Ok(TimeoutRecoveryOutcome::Reset) => {
                let dropped_reverse_lanes = if let Some(reverse) = &self.reverse_connection {
                    reverse.reset_idle_connections().await
                } else {
                    0
                };
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    operation,
                    dropped_reverse_lanes,
                    "reset daemon connection after timeout"
                );
                "connection was reset; request was not replayed".to_string()
            }
            Err(err) => {
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    operation,
                    error = %err,
                    "failed to reset daemon connection after timeout"
                );
                format!("connection reset failed: {err}; request was not replayed")
            }
        }
    }

    #[cfg(test)]
    pub(super) fn request(&self, path: &str) -> reqwest::RequestBuilder {
        let connection = self.connection.snapshot();
        self.request_with_client(&connection.client, path)
    }

    pub(super) fn request_with_generation(&self, path: &str) -> (reqwest::RequestBuilder, u64) {
        let connection = self.connection.snapshot();
        (
            self.request_with_client(&connection.client, path),
            connection.generation,
        )
    }

    pub(super) fn record_connection_success(&self, generation: u64) {
        self.connection.record_success(generation);
    }

    fn request_with_client(&self, client: &reqwest::Client, path: &str) -> reqwest::RequestBuilder {
        let request_id = crate::request_context::current_request_id().unwrap_or_default();
        let mut request = client
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
