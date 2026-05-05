mod support;

use base64::Engine;
use remote_exec_proto::rpc::{
    EmptyResponse, PortConnectionCloseRequest, PortConnectionReadRequest,
    PortConnectionReadResponse, PortConnectionWriteRequest, PortForwardProtocol,
    PortListenAcceptRequest, PortListenRequest, PortListenResponse, PortUdpDatagramReadRequest,
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
async fn port_forward_pending_udp_read_returns_closed_after_listen_close() {
    let fixture = support::spawn::spawn_daemon("builder-a").await;
    let listen = fixture
        .client
        .post(fixture.url("/v1/port/listen"))
        .json(&PortListenRequest {
            endpoint: "127.0.0.1:0".to_string(),
            protocol: PortForwardProtocol::Udp,
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
