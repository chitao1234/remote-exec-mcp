use futures_util::TryStreamExt;

use remote_exec_proto::port_tunnel::{
    TUNNEL_PROTOCOL_VERSION, TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN, write_preface,
};
use remote_exec_proto::request_id::REQUEST_ID_HEADER;
use remote_exec_proto::rpc::{
    ExecResponse, ExecStartRequest, ExecWriteRequest, ImageReadRequest, ImageReadResponse,
    PatchApplyRequest, PatchApplyResponse, RpcErrorBody, RpcErrorCode, TargetInfoResponse,
    TransferSourceType,
};
use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_LENGTH, HeaderValue, UPGRADE};

use crate::config::{TargetConfig, TargetTimeoutConfig, TargetTransportKind};

// Upgrade covers the HTTP 101 handshake and reqwest's transition into raw I/O.
const PORT_TUNNEL_UPGRADE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
// Preface covers only the v4 tunnel greeting after the HTTP upgrade succeeds.
const PORT_TUNNEL_PREFACE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
        code: Option<DaemonRpcCode>,
        message: String,
    },
    Decode(anyhow::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonRpcCode {
    Known(RpcErrorCode),
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcToolErrorMode {
    Full,
    MessageOnly,
}

impl DaemonRpcCode {
    pub fn from_wire_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match RpcErrorCode::from_wire_value(&value) {
            Some(code) => Self::Known(code),
            None => Self::Unknown(value),
        }
    }

    pub fn as_wire_value(&self) -> &str {
        match self {
            Self::Known(code) => code.wire_value(),
            Self::Unknown(value) => value.as_str(),
        }
    }

    pub fn known(&self) -> Option<RpcErrorCode> {
        match self {
            Self::Known(code) => Some(*code),
            Self::Unknown(_) => None,
        }
    }
}

impl DaemonClientError {
    pub fn rpc_code(&self) -> Option<&str> {
        match self {
            Self::Rpc { code, .. } => code.as_ref().map(DaemonRpcCode::as_wire_value),
            _ => None,
        }
    }

    pub fn rpc_error_code(&self) -> Option<RpcErrorCode> {
        match self {
            Self::Rpc { code, .. } => code.as_ref().and_then(DaemonRpcCode::known),
            _ => None,
        }
    }

    pub fn is_rpc_error_code(&self, expected: RpcErrorCode) -> bool {
        self.rpc_error_code() == Some(expected)
    }

    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport(_))
    }

    pub fn into_tool_error(self, rpc_mode: RpcToolErrorMode) -> anyhow::Error {
        match (self, rpc_mode) {
            (Self::Rpc { message, .. }, RpcToolErrorMode::MessageOnly) => {
                anyhow::Error::msg(message)
            }
            (other, _) => other.into(),
        }
    }
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "daemon transport error: {err:#}"),
            Self::Decode(err) => {
                let message = err.to_string();
                if message.starts_with("daemon returned malformed exec response: ") {
                    f.write_str(&message)
                } else {
                    write!(f, "daemon decode error: {message}")
                }
            }
            Self::Rpc {
                status,
                code,
                message,
            } => match code {
                Some(code) => write!(f, "{}: {message} ({status})", code.as_wire_value()),
                None => write!(f, "daemon returned {status}: {message}"),
            },
        }
    }
}

impl std::error::Error for DaemonClientError {}

#[derive(Clone, Copy)]
enum RpcLogSubject<'a> {
    Path(&'a str),
    DestinationPath(&'a str),
}

#[derive(Clone, Copy)]
enum RpcCallKind {
    Rpc,
    TransferExport,
    TransferImport,
}

#[derive(Clone, Copy)]
struct RpcCallContext<'a> {
    target_name: &'a str,
    base_url: &'a str,
    started: std::time::Instant,
    kind: RpcCallKind,
    subject: RpcLogSubject<'a>,
}

impl RpcCallKind {
    fn label(self, suffix: &str) -> String {
        let prefix = match self {
            Self::Rpc => "daemon rpc",
            Self::TransferExport => "daemon transfer export",
            Self::TransferImport => "daemon transfer import",
        };
        format!("{prefix} {suffix}")
    }
}

macro_rules! log_rpc {
    ($level:ident, $ctx:expr, $msg:expr $(, $($field:tt)*)?) => {{
        let elapsed_ms = $ctx.started.elapsed().as_millis() as u64;
        match $ctx.subject {
            RpcLogSubject::Path(path) => tracing::$level!(
                target = %$ctx.target_name,
                base_url = %$ctx.base_url,
                path,
                elapsed_ms,
                $($($field)*)?
                "{}", $msg
            ),
            RpcLogSubject::DestinationPath(destination_path) => tracing::$level!(
                target = %$ctx.target_name,
                base_url = %$ctx.base_url,
                destination_path,
                elapsed_ms,
                $($($field)*)?
                "{}", $msg
            ),
        }
    }};
}

impl<'a> RpcCallContext<'a> {
    fn path(
        target_name: &'a str,
        base_url: &'a str,
        started: std::time::Instant,
        kind: RpcCallKind,
        path: &'a str,
    ) -> Self {
        Self {
            target_name,
            base_url,
            started,
            kind,
            subject: RpcLogSubject::Path(path),
        }
    }

    fn destination_path(
        target_name: &'a str,
        base_url: &'a str,
        started: std::time::Instant,
        kind: RpcCallKind,
        destination_path: &'a str,
    ) -> Self {
        Self {
            target_name,
            base_url,
            started,
            kind,
            subject: RpcLogSubject::DestinationPath(destination_path),
        }
    }

    fn log_completed(self) {
        log_rpc!(debug, self, self.kind.label("completed"));
    }

    fn log_transport_error(self, err: &reqwest::Error) {
        log_rpc!(warn, self, self.kind.label("transport failed"), error = %err,);
    }

    fn log_status_error(self, status: reqwest::StatusCode) {
        log_rpc!(
            warn,
            self,
            self.kind.label("returned error status"),
            status = status.as_u16(),
        );
    }

    fn log_read_error(self, err: &reqwest::Error) {
        log_rpc!(warn, self, self.kind.label("body read failed"), error = %err,);
    }

    fn log_decode_error(self, err: &serde_json::Error) {
        log_rpc!(warn, self, self.kind.label("decode failed"), error = %err,);
    }
}

#[derive(Clone)]
pub struct DaemonClient {
    client: reqwest::Client,
    target_name: String,
    base_url: String,
    authorization: Option<HeaderValue>,
    request_timeout: std::time::Duration,
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

    pub async fn port_tunnel(&self) -> Result<reqwest::Upgraded, DaemonClientError> {
        let started = std::time::Instant::now();
        let request = self
            .request("/v1/port/tunnel")
            .header(CONNECTION, "Upgrade")
            .header(UPGRADE, UPGRADE_TOKEN)
            .header(TUNNEL_PROTOCOL_VERSION_HEADER, TUNNEL_PROTOCOL_VERSION)
            .header(CONTENT_LENGTH, "0");
        let response = tokio::time::timeout(PORT_TUNNEL_UPGRADE_TIMEOUT, request.send())
            .await
            .map_err(|_| {
                DaemonClientError::Transport(anyhow::anyhow!("port tunnel upgrade timed out"))
            })?
            .map_err(|err| self.rpc_transport_error("/v1/port/tunnel", started, err))?;
        if response.status() != reqwest::StatusCode::SWITCHING_PROTOCOLS {
            return Err(decode_rpc_error(response, RpcErrorDecodePolicy::Lenient).await);
        }
        let mut upgraded = tokio::time::timeout(PORT_TUNNEL_UPGRADE_TIMEOUT, response.upgrade())
            .await
            .map_err(|_| {
                DaemonClientError::Transport(anyhow::anyhow!("port tunnel upgrade timed out"))
            })?
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        tokio::time::timeout(PORT_TUNNEL_PREFACE_TIMEOUT, write_preface(&mut upgraded))
            .await
            .map_err(|_| {
                DaemonClientError::Transport(anyhow::anyhow!("port tunnel preface timed out"))
            })?
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Ok(upgraded)
    }

    async fn post<Req, Resp>(&self, path: &str, body: &Req) -> Result<Resp, DaemonClientError>
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
            let response = self
                .send_request_with_policy(
                    self.request(path).json(body).send(),
                    RpcErrorDecodePolicy::Strict,
                    |err| context.log_transport_error(err),
                    |status| context.log_status_error(status),
                )
                .await?;

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
                tracing::warn!(
                    target = %self.target_name,
                    base_url = %self.base_url,
                    path,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    timeout_ms,
                    "daemon rpc timed out"
                );
                Err(DaemonClientError::Transport(anyhow::anyhow!(
                    "daemon rpc `{path}` timed out after {timeout_ms} ms"
                )))
            }
        }
    }

    async fn send_request_with_policy<Send, LogTransport, LogStatus>(
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

    async fn decode_json_response<Resp, LogRead, LogDecode>(
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
        match decode_policy {
            RpcErrorDecodePolicy::Strict => {
                Err(decode_rpc_error(response, RpcErrorDecodePolicy::Strict).await)
            }
            RpcErrorDecodePolicy::Lenient => {
                Err(decode_rpc_error(response, RpcErrorDecodePolicy::Lenient).await)
            }
        }
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

#[derive(Clone, Copy)]
enum RpcErrorDecodePolicy {
    Strict,
    Lenient,
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

pub(crate) fn normalize_tool_error(
    err: anyhow::Error,
    rpc_mode: RpcToolErrorMode,
) -> anyhow::Error {
    match err.downcast::<DaemonClientError>() {
        Ok(other) => other.into_tool_error(rpc_mode),
        Err(other) => other,
    }
}

pub(crate) fn normalize_tool_result<T>(
    result: Result<T, DaemonClientError>,
    rpc_mode: RpcToolErrorMode,
) -> anyhow::Result<T> {
    result.map_err(|err| err.into_tool_error(rpc_mode))
}

async fn decode_rpc_error(
    response: reqwest::Response,
    decode_policy: RpcErrorDecodePolicy,
) -> DaemonClientError {
    let status = response.status();
    match response.text().await {
        Ok(body) => decode_rpc_error_body(status, body),
        Err(err) => match decode_policy {
            RpcErrorDecodePolicy::Strict => DaemonClientError::Transport(err.into()),
            RpcErrorDecodePolicy::Lenient => decode_rpc_error_body(status, err.to_string()),
        },
    }
}

fn decode_rpc_error_body(status: reqwest::StatusCode, body: String) -> DaemonClientError {
    if let Ok(error) = serde_json::from_str::<RpcErrorBody>(&body) {
        DaemonClientError::Rpc {
            status,
            code: Some(DaemonRpcCode::from_wire_value(error.wire_code())),
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

mod transfer;

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use remote_exec_proto::request_id::{REQUEST_ID_HEADER, RequestId};

    use super::*;
    use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_client(authorization: Option<HeaderValue>) -> DaemonClient {
        crate::install_crypto_provider().unwrap();
        DaemonClient {
            client: reqwest::Client::builder().build().unwrap(),
            target_name: DEFAULT_TEST_TARGET.to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            authorization,
            request_timeout: crate::config::TargetTimeoutConfig::default().request_timeout(),
        }
    }

    async fn hung_response_client(
        timeout: Duration,
    ) -> (DaemonClient, tokio::task::JoinHandle<()>) {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client = DaemonClient {
            client: reqwest::Client::builder().build().unwrap(),
            target_name: DEFAULT_TEST_TARGET.to_string(),
            base_url: format!("http://{addr}"),
            authorization: None,
            request_timeout: timeout,
        };

        (client, server)
    }

    #[tokio::test]
    async fn daemon_rpc_times_out_hung_response() {
        let (client, server) = hung_response_client(Duration::from_millis(50)).await;

        let started = std::time::Instant::now();
        let err = client.target_info().await.unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "timeout took too long: {:?}",
            started.elapsed()
        );
        assert!(
            err.to_string()
                .contains("daemon rpc `/v1/target-info` timed out after 50 ms"),
            "unexpected error: {err}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn daemon_exec_rpc_times_out_hung_exec_start_response() {
        let (client, server) = hung_response_client(Duration::from_millis(50)).await;

        let err = client
            .exec_start(&ExecStartRequest {
                cmd: "sleep 30".to_string(),
                workdir: None,
                shell: None,
                tty: false,
                yield_time_ms: None,
                max_output_tokens: None,
                login: None,
            })
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("daemon rpc `/v1/exec/start` timed out after 50 ms"),
            "unexpected error: {err}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn daemon_exec_rpc_times_out_hung_exec_write_response() {
        let (client, server) = hung_response_client(Duration::from_millis(50)).await;

        let err = client
            .exec_write(&ExecWriteRequest {
                daemon_session_id: "daemon-session-1".to_string(),
                chars: String::new(),
                yield_time_ms: None,
                max_output_tokens: None,
                pty_size: None,
            })
            .await
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("daemon rpc `/v1/exec/write` timed out after 50 ms"),
            "unexpected error: {err}"
        );
        server.abort();
    }

    #[test]
    fn daemon_request_does_not_force_connection_close() {
        let request = test_client(None)
            .request("/v1/target-info")
            .build()
            .unwrap();

        assert!(
            request.headers().get(reqwest::header::CONNECTION).is_none(),
            "broker daemon client should let reqwest manage persistent connections"
        );
    }

    #[test]
    fn daemon_request_includes_generated_request_id_header() {
        let request = test_client(None)
            .request("/v1/target-info")
            .build()
            .unwrap();

        let request_id = request
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("request id header should be present");
        assert!(RequestId::from_header_value(request_id).is_some());
    }

    #[tokio::test]
    async fn daemon_request_reuses_current_request_context_id() {
        let context = crate::request_context::RequestContext::new("test_tool");
        let expected_request_id = context.request_id().to_string();
        let request = crate::request_context::scope(context, async {
            test_client(None)
                .request("/v1/target-info")
                .build()
                .unwrap()
        })
        .await;

        assert_eq!(
            request
                .headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some(expected_request_id.as_str())
        );
    }

    #[test]
    fn daemon_request_still_applies_authorization_header() {
        let request = test_client(Some(HeaderValue::from_static("Bearer shared-secret")))
            .request("/v1/target-info")
            .build()
            .unwrap();

        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer shared-secret")
        );
        assert!(request.headers().get(reqwest::header::CONNECTION).is_none());
    }

    #[test]
    fn rpc_error_code_classifies_known_wire_values() {
        let err = decode_rpc_error_body(
            reqwest::StatusCode::NOT_FOUND,
            serde_json::json!({
                "code": RpcErrorCode::UnknownEndpoint.wire_value(),
                "message": "unsupported"
            })
            .to_string(),
        );

        assert_eq!(err.rpc_code(), Some("unknown_endpoint"));
        assert_eq!(err.rpc_error_code(), Some(RpcErrorCode::UnknownEndpoint));
        assert!(err.is_rpc_error_code(RpcErrorCode::UnknownEndpoint));
        assert!(!err.is_rpc_error_code(RpcErrorCode::NotFound));
    }

    #[test]
    fn rpc_error_code_leaves_unknown_wire_values_unclassified() {
        let err = decode_rpc_error_body(
            reqwest::StatusCode::BAD_REQUEST,
            serde_json::json!({
                "code": "future_error_code",
                "message": "newer daemon"
            })
            .to_string(),
        );

        assert_eq!(err.rpc_code(), Some("future_error_code"));
        assert_eq!(err.rpc_error_code(), None);
    }

    #[tokio::test]
    async fn port_tunnel_sends_upgrade_headers_and_preface() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                let read = stream.read(&mut byte).await.unwrap();
                request.extend_from_slice(&byte[..read]);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("POST /v1/port/tunnel HTTP/1.1\r\n"));
            assert!(request.to_ascii_lowercase().contains("connection: upgrade"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("upgrade: remote-exec-port-tunnel")
            );
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-remote-exec-port-tunnel-version: 4")
            );
            assert!(request.to_ascii_lowercase().contains("x-request-id: req_"));

            stream
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: remote-exec-port-tunnel\r\n\r\n",
                )
                .await
                .unwrap();
            let mut preface = [0u8; 8];
            let mut filled = 0;
            while filled < preface.len() {
                let read = stream.read(&mut preface[filled..]).await.unwrap();
                filled += read;
            }
            assert_eq!(&preface, b"REPFWD1\n");
        });

        let upgraded = DaemonClient {
            client: reqwest::Client::builder().build().unwrap(),
            target_name: DEFAULT_TEST_TARGET.to_string(),
            base_url: format!("http://{addr}"),
            authorization: None,
            request_timeout: crate::config::TargetTimeoutConfig::default().request_timeout(),
        }
        .port_tunnel()
        .await
        .unwrap();
        drop(upgraded);
        server.await.unwrap();
    }
}
