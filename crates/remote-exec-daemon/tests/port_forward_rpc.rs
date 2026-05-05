mod support;

use std::sync::Arc;

use base64::Engine;
use remote_exec_daemon::config::{DaemonConfig, DaemonTransport, ProcessEnvironment, PtyMode};
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectionCloseRequest, PortConnectionReadRequest,
    PortConnectionReadResponse, PortConnectionWriteRequest, PortForwardLease, PortForwardProtocol,
    PortLeaseRenewRequest, PortListenAcceptRequest, PortListenRequest, PortListenResponse,
    PortUdpDatagramReadRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn port_forward_listen_normalizes_bare_port_and_closes() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let response = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: "0".to_string(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let listen = response.json::<PortListenResponse>().await.unwrap();
    assert!(
        listen.endpoint.starts_with("127.0.0.1:"),
        "unexpected endpoint {}",
        listen.endpoint
    );
    assert_ne!(listen.endpoint, "127.0.0.1:0");

    let close = fixture
        .client
        .post(fixture.url("/v1/port/listen/close"))
        .json(&remote_exec_proto::rpc::PortListenCloseRequest {
            bind_id: listen.bind_id,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(close.status(), reqwest::StatusCode::OK);
    close.json::<EmptyResponse>().await.unwrap();
}

#[tokio::test]
async fn port_forward_accept_read_write_and_close_tcp_connection() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let listen = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        })
        .send()
        .await
        .unwrap()
        .json::<PortListenResponse>()
        .await
        .unwrap();

    let client = fixture.client.clone();
    let accept_url = fixture.url("/v1/port/listen/accept");
    let bind_id = listen.bind_id.clone();
    let accept_task = tokio::spawn(async move {
        client
            .post(accept_url)
            .json(&PortListenAcceptRequest { bind_id })
            .send()
            .await
            .unwrap()
            .json::<remote_exec_proto::rpc::PortListenAcceptResponse>()
            .await
            .unwrap()
    });

    let mut stream = tokio::net::TcpStream::connect(&listen.endpoint)
        .await
        .unwrap();
    let accepted = accept_task.await.unwrap();
    stream.write_all(b"ping").await.unwrap();

    let read = fixture
        .client
        .post(fixture.url("/v1/port/connection/read"))
        .json(&PortConnectionReadRequest {
            connection_id: accepted.connection_id.clone(),
        })
        .send()
        .await
        .unwrap()
        .json::<PortConnectionReadResponse>()
        .await
        .unwrap();
    assert!(!read.eof);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(read.data)
            .unwrap(),
        b"ping"
    );

    fixture
        .client
        .post(fixture.url("/v1/port/connection/write"))
        .json(&PortConnectionWriteRequest {
            connection_id: accepted.connection_id.clone(),
            data: base64::engine::general_purpose::STANDARD.encode(b"pong"),
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"pong");

    fixture
        .client
        .post(fixture.url("/v1/port/connection/close"))
        .json(&PortConnectionCloseRequest {
            connection_id: accepted.connection_id,
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();
}

#[tokio::test]
async fn port_forward_pending_accept_returns_closed_after_listen_close() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let listen = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        })
        .send()
        .await
        .unwrap()
        .json::<PortListenResponse>()
        .await
        .unwrap();

    let client = fixture.client.clone();
    let accept_url = fixture.url("/v1/port/listen/accept");
    let bind_id = listen.bind_id.clone();
    let accept_task = tokio::spawn(async move {
        client
            .post(accept_url)
            .json(&PortListenAcceptRequest { bind_id })
            .send()
            .await
            .unwrap()
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    fixture
        .client
        .post(fixture.url("/v1/port/listen/close"))
        .json(&remote_exec_proto::rpc::PortListenCloseRequest {
            bind_id: listen.bind_id,
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), accept_task)
        .await
        .expect("accept should return after close")
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "port_bind_closed");
}

#[tokio::test]
async fn port_forward_pending_accept_returns_closed_after_daemon_shutdown() {
    let tempdir = tempfile::tempdir().unwrap();
    let workdir = tempdir.path().join("workdir");
    std::fs::create_dir_all(&workdir).unwrap();
    let state = Arc::new(
        remote_exec_daemon::build_app_state(DaemonConfig {
            target: "builder-a".to_string(),
            listen: "127.0.0.1:0".parse().unwrap(),
            default_workdir: workdir,
            windows_posix_root: None,
            transport: DaemonTransport::Http,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            allow_login_shell: true,
            pty: PtyMode::Auto,
            default_shell: None,
            yield_time: remote_exec_daemon::config::YieldTimeConfig::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: ProcessEnvironment::capture_current(),
            tls: None,
        })
        .unwrap(),
    );

    let listen = remote_exec_host::port_forward::listen_local(
        state.clone(),
        PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        },
    )
    .await
    .unwrap();

    let accept_state = state.clone();
    let bind_id = listen.bind_id.clone();
    let accept_task = tokio::spawn(async move {
        remote_exec_host::port_forward::listen_accept_local(
            accept_state,
            PortListenAcceptRequest { bind_id },
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    remote_exec_host::port_forward::shutdown_local(&state).await;

    let err = tokio::time::timeout(std::time::Duration::from_secs(2), accept_task)
        .await
        .expect("accept should return after daemon shutdown")
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, "port_bind_closed");
}

#[tokio::test]
async fn port_forward_pending_udp_read_returns_closed_after_listen_close() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let listen = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: PortForwardProtocol::Udp,
            lease: None,
        })
        .send()
        .await
        .unwrap()
        .json::<PortListenResponse>()
        .await
        .unwrap();

    let client = fixture.client.clone();
    let read_url = fixture.url("/v1/port/udp/read");
    let bind_id = listen.bind_id.clone();
    let read_task = tokio::spawn(async move {
        client
            .post(read_url)
            .json(&PortUdpDatagramReadRequest { bind_id })
            .send()
            .await
            .unwrap()
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    fixture
        .client
        .post(fixture.url("/v1/port/listen/close"))
        .json(&remote_exec_proto::rpc::PortListenCloseRequest {
            bind_id: listen.bind_id,
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), read_task)
        .await
        .expect("udp read should return after close")
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "port_bind_closed");
}

#[tokio::test]
async fn port_forward_pending_connection_read_returns_closed_after_connection_close() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let listen = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        })
        .send()
        .await
        .unwrap()
        .json::<PortListenResponse>()
        .await
        .unwrap();

    let client = fixture.client.clone();
    let accept_url = fixture.url("/v1/port/listen/accept");
    let bind_id = listen.bind_id.clone();
    let accept_task = tokio::spawn(async move {
        client
            .post(accept_url)
            .json(&PortListenAcceptRequest { bind_id })
            .send()
            .await
            .unwrap()
            .json::<remote_exec_proto::rpc::PortListenAcceptResponse>()
            .await
            .unwrap()
    });

    let _stream = tokio::net::TcpStream::connect(&listen.endpoint)
        .await
        .unwrap();
    let accepted = accept_task.await.unwrap();

    let client = fixture.client.clone();
    let read_url = fixture.url("/v1/port/connection/read");
    let connection_id = accepted.connection_id.clone();
    let read_task = tokio::spawn(async move {
        client
            .post(read_url)
            .json(&PortConnectionReadRequest { connection_id })
            .send()
            .await
            .unwrap()
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    fixture
        .client
        .post(fixture.url("/v1/port/connection/close"))
        .json(&PortConnectionCloseRequest {
            connection_id: accepted.connection_id,
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), read_task)
        .await
        .expect("connection read should return after close")
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let err = response
        .json::<remote_exec_proto::rpc::RpcErrorBody>()
        .await
        .unwrap();
    assert_eq!(err.code, "port_connection_closed");
}

#[tokio::test]
async fn leased_port_forward_listener_can_be_reclaimed_after_expiry() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = reserved.local_addr().unwrap().to_string();
    drop(reserved);

    let response = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: endpoint.clone(),
            protocol: PortForwardProtocol::Tcp,
            lease: Some(PortForwardLease {
                lease_id: "lease-test".to_string(),
                ttl_ms: 300,
            }),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let listen = response.json::<PortListenResponse>().await.unwrap();
    assert_eq!(listen.endpoint, endpoint);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let rebound = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: endpoint.clone(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), reqwest::StatusCode::OK);
    let rebound = rebound.json::<PortListenResponse>().await.unwrap();
    assert_eq!(rebound.endpoint, endpoint);

    fixture
        .client
        .post(fixture.url("/v1/port/listen/close"))
        .json(&remote_exec_proto::rpc::PortListenCloseRequest {
            bind_id: rebound.bind_id,
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();
}

#[tokio::test]
async fn late_port_forward_lease_renew_is_ignored_after_expiry() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = reserved.local_addr().unwrap().to_string();
    drop(reserved);

    let response = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: endpoint.clone(),
            protocol: PortForwardProtocol::Tcp,
            lease: Some(PortForwardLease {
                lease_id: "lease-late-renew".to_string(),
                ttl_ms: 300,
            }),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let listen = response.json::<PortListenResponse>().await.unwrap();
    assert_eq!(listen.endpoint, endpoint);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let renew = fixture
        .client
        .post(fixture.url("/v1/port/lease/renew"))
        .json(&PortLeaseRenewRequest {
            lease_id: "lease-late-renew".to_string(),
            ttl_ms: 300,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(renew.status(), reqwest::StatusCode::OK);
    renew.json::<EmptyResponse>().await.unwrap();

    let rebound = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: endpoint.clone(),
            protocol: PortForwardProtocol::Tcp,
            lease: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(rebound.status(), reqwest::StatusCode::OK);
    let rebound = rebound.json::<PortListenResponse>().await.unwrap();
    assert_eq!(rebound.endpoint, endpoint);

    fixture
        .client
        .post(fixture.url("/v1/port/listen/close"))
        .json(&remote_exec_proto::rpc::PortListenCloseRequest {
            bind_id: rebound.bind_id,
        })
        .send()
        .await
        .unwrap()
        .json::<EmptyResponse>()
        .await
        .unwrap();
}
