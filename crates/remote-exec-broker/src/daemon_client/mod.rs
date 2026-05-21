mod client;
mod error;
mod logging;
mod response;
mod transfer;
mod transfer_stream;
mod tunnel;

pub use client::DaemonClient;
pub use error::{DaemonClientError, DaemonRpcCode, RpcToolErrorMode};
pub use transfer::{TransferExportResponse, TransferExportStream};

pub(crate) use client::apply_daemon_client_timeouts;
pub(crate) use error::{normalize_tool_error, normalize_tool_result};
pub(in crate::daemon_client) use logging::{RpcCallContext, RpcCallKind};
pub(in crate::daemon_client) use response::{RpcErrorDecodePolicy, decode_rpc_error};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use remote_exec_proto::request_id::{REQUEST_ID_HEADER, RequestId};
    use remote_exec_proto::rpc::{ExecStartRequest, ExecWriteRequest, RpcErrorCode};
    use reqwest::header::HeaderValue;

    use super::response::decode_rpc_error_body;
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
