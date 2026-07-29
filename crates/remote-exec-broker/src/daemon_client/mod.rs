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

#[cfg(feature = "broker-tls")]
pub(crate) use client::apply_daemon_client_timeouts;
pub(crate) use error::{normalize_tool_error, normalize_tool_result};
pub(in crate::daemon_client) use logging::{RpcCallContext, RpcCallKind};
pub(in crate::daemon_client) use response::{RpcErrorDecodePolicy, decode_rpc_error};

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use remote_exec_proto::request_id::{REQUEST_ID_HEADER, RequestId};
    use remote_exec_proto::rpc::{
        DaemonIdentity, ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecStartRequest,
        ExecWriteRequest, RpcErrorCode, TargetCapabilities, TargetInfoResponse,
        TransferStreamProtocolVersion,
    };
    use reqwest::header::HeaderValue;

    use super::response::decode_rpc_error_body;
    use super::*;
    use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_client(authorization: Option<HeaderValue>) -> DaemonClient {
        crate::install_crypto_provider().unwrap();
        DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            "http://127.0.0.1:9".to_string(),
            authorization,
            crate::config::TargetTimeoutConfig::default().request_timeout(),
            crate::config::TargetTimeoutConfig::default().startup_probe_timeout(),
        )
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

        let client = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            timeout,
            timeout,
        );

        (client, server)
    }

    async fn delayed_response_client(
        request_timeout: Duration,
        delay: Duration,
        response_body: String,
    ) -> (DaemonClient, tokio::task::JoinHandle<()>) {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_http_request(&mut stream).await;
            tokio::time::sleep(delay).await;
            write_json_response(&mut stream, &response_body).await;
        });

        let client = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            request_timeout,
            request_timeout,
        );

        (client, server)
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut data = Vec::new();
        loop {
            let mut buf = [0u8; 512];
            let read = stream.read(&mut buf).await.unwrap();
            assert!(read > 0, "client closed before sending a full request");
            data.extend_from_slice(&buf[..read]);

            let Some(header_end) = data.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&data[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if data.len() >= header_end + 4 + content_length {
                return String::from_utf8_lossy(&data).into_owned();
            }
        }
    }

    fn target_info_response_body() -> String {
        serde_json::to_string(&TargetInfoResponse {
            target: DEFAULT_TEST_TARGET.to_string(),
            daemon_instance_id: "daemon-retry".to_string(),
            identity: DaemonIdentity {
                daemon_version: "test".to_string(),
                hostname: "host".to_string(),
                platform: "linux".to_string(),
                arch: "x86_64".to_string(),
            },
            capabilities: TargetCapabilities {
                supports_pty: true,
                supports_port_forward: false,
                port_forward_protocol_version: None,
                transfer_stream_protocol_version: Some(TransferStreamProtocolVersion::v2()),
                file_tool_protocol_version: None,
            },
            supports_image_read: true,
            supports_transfer_compression: false,
        })
        .unwrap()
    }

    async fn write_json_response(stream: &mut tokio::net::TcpStream, body: &str) {
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    fn completed_exec_response_body() -> String {
        serde_json::to_string(&ExecResponse::Completed(ExecCompletedResponse {
            output: ExecOutputResponse {
                daemon_instance_id: "daemon-1".to_string(),
                running: false,
                chunk_id: Some("chunk-1".to_string()),
                wall_time_seconds: 0.1,
                exit_code: Some(0),
                original_token_count: Some(1),
                output: "ok\n".to_string(),
                warnings: Vec::new(),
            },
        }))
        .unwrap()
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
        assert!(
            err.to_string().contains(
                "connection was retained after 1 consecutive timeout; request was not replayed"
            ),
            "unexpected recovery message: {err}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn consecutive_direct_timeouts_reset_connection_without_replay() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut hung_connections = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                assert!(request.starts_with("POST /v1/target-info "));
                hung_connections.push(tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    drop(stream);
                }));
            }
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            assert!(request.starts_with("POST /v1/target-info "));
            write_json_response(&mut stream, &target_info_response_body()).await;
            for connection in hung_connections {
                connection.await.unwrap();
            }
        });
        let client = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            Duration::from_millis(50),
            Duration::from_millis(50),
        );

        let first = client.target_info().await.unwrap_err();
        assert!(first.to_string().contains("connection was retained"));
        let second = client.target_info().await.unwrap_err();
        assert!(second.to_string().contains("connection was reset"));
        assert!(second.to_string().contains("request was not replayed"));

        let recovered = client.target_info().await.unwrap();
        assert_eq!(recovered.daemon_instance_id, "daemon-retry");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn successful_response_clears_direct_timeout_streak() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut first).await;
            assert!(request.starts_with("POST /v1/target-info "));
            let first_hung = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                drop(first);
            });

            let (mut second, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut second).await;
            assert!(request.starts_with("POST /v1/target-info "));
            write_json_response(&mut second, &target_info_response_body()).await;
            drop(second);

            let (mut third, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut third).await;
            assert!(request.starts_with("POST /v1/target-info "));
            tokio::time::sleep(Duration::from_millis(100)).await;
            first_hung.await.unwrap();
        });
        let client = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            Duration::from_millis(50),
            Duration::from_millis(50),
        );

        let first = client.target_info().await.unwrap_err();
        assert!(first.to_string().contains("connection was retained"));
        client.target_info().await.unwrap();
        let third = client.target_info().await.unwrap_err();
        assert!(third.to_string().contains("connection was retained"));
        assert!(!third.to_string().contains("connection was reset;"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn response_headers_without_body_do_not_clear_timeout_streak() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut stalled_connections = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                assert!(request.starts_with("POST /v1/target-info "));
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1024\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stalled_connections.push(stream);
            }

            let (mut recovered, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut recovered).await;
            assert!(request.starts_with("POST /v1/target-info "));
            write_json_response(&mut recovered, &target_info_response_body()).await;
            drop(stalled_connections);
        });
        let client = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            Duration::from_millis(50),
            Duration::from_millis(50),
        );

        let first = client.target_info().await.unwrap_err();
        assert!(first.to_string().contains("connection was retained"));
        let second = client.target_info().await.unwrap_err();
        assert!(second.to_string().contains("connection was reset"));
        assert!(second.to_string().contains("request was not replayed"));

        let recovered = client.target_info().await.unwrap();
        assert_eq!(recovered.daemon_instance_id, "daemon-retry");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn response_body_read_timeouts_reset_connection_without_replay() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut stalled_connections = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_http_request(&mut stream).await;
                assert!(request.starts_with("POST /v1/target-info "));
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1024\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stalled_connections.push(stream);
            }

            let (mut recovered, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut recovered).await;
            assert!(request.starts_with("POST /v1/target-info "));
            write_json_response(&mut recovered, &target_info_response_body()).await;
            drop(stalled_connections);
        });
        let defaults = crate::config::TargetTimeoutConfig::default();
        let client = DaemonClient::new(
            DEFAULT_TEST_TARGET,
            &crate::config::TargetConfig {
                base_url: format!("http://{addr}"),
                http_auth: None,
                timeouts: crate::config::TargetTimeoutConfig {
                    connect_ms: defaults.connect_ms,
                    read_ms: 50,
                    request_ms: 500,
                    startup_probe_ms: 500,
                },
                ca_pem: None,
                client_cert_pem: None,
                client_key_pem: None,
                allow_insecure_http: true,
                skip_server_name_verification: false,
                pinned_server_cert_pem: None,
                expected_daemon_name: None,
            },
            None,
        )
        .await
        .unwrap();

        let first = client.target_info().await.unwrap_err();
        assert!(first.to_string().contains("connection was retained"));
        let second = client.target_info().await.unwrap_err();
        assert!(second.to_string().contains("connection was reset"));
        assert!(second.to_string().contains("request was not replayed"));

        let recovered = client.target_info().await.unwrap();
        assert_eq!(recovered.daemon_instance_id, "daemon-retry");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn daemon_target_info_retries_closed_request_transport_once() {
        crate::install_crypto_provider().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let first_request = read_http_request(&mut first).await;
            assert!(first_request.starts_with("POST /v1/target-info "));
            drop(first);

            let (mut second, _) = listener.accept().await.unwrap();
            let second_request = read_http_request(&mut second).await;
            assert!(second_request.starts_with("POST /v1/target-info "));
            write_json_response(&mut second, &target_info_response_body()).await;
        });

        let client = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            Duration::from_secs(5),
            Duration::from_secs(5),
        );

        let info = client.target_info().await.unwrap();
        assert_eq!(info.daemon_instance_id, "daemon-retry");
        server.await.unwrap();
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

    #[tokio::test]
    async fn daemon_exec_start_timeout_accounts_for_requested_yield_time() {
        let (client, server) = delayed_response_client(
            Duration::from_millis(50),
            Duration::from_millis(125),
            completed_exec_response_body(),
        )
        .await;

        let response = client
            .exec_start(&ExecStartRequest {
                cmd: "slow command".to_string(),
                workdir: None,
                shell: None,
                tty: false,
                yield_time_ms: Some(100),
                max_output_tokens: None,
                login: None,
            })
            .await
            .unwrap();

        assert!(!response.running());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn daemon_exec_write_timeout_accounts_for_requested_yield_time() {
        let (client, server) = delayed_response_client(
            Duration::from_millis(50),
            Duration::from_millis(125),
            completed_exec_response_body(),
        )
        .await;

        let response = client
            .exec_write(&ExecWriteRequest {
                daemon_session_id: "daemon-session-1".to_string(),
                chars: String::new(),
                yield_time_ms: Some(100),
                max_output_tokens: None,
                pty_size: None,
            })
            .await
            .unwrap();

        assert!(!response.running());
        server.await.unwrap();
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

        let upgraded = DaemonClient::from_test_client(
            reqwest::Client::builder().build().unwrap(),
            format!("http://{addr}"),
            None,
            crate::config::TargetTimeoutConfig::default().request_timeout(),
            crate::config::TargetTimeoutConfig::default().startup_probe_timeout(),
        )
        .port_tunnel()
        .await
        .unwrap();
        drop(upgraded);
        server.await.unwrap();
    }
}
