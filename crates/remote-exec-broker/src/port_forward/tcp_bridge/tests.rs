use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use std::time::Duration;

use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::port_tunnel::{
    ForwardDropKind, ForwardDropMeta, Frame, FrameType, read_frame, write_frame,
};
use remote_exec_proto::public::ForwardPortProtocol as PublicForwardPortProtocol;
use remote_exec_proto::rpc::RpcErrorCode;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::sync::CancellationToken;

use super::super::epoch::{ForwardEpoch, INITIAL_FORWARD_GENERATION};
use super::super::side::SideHandle;
use super::super::supervisor::{
    ForwardIdentity, ForwardLimits, ForwardRuntime, ListenSessionControl,
};
use super::super::test_support::{
    ScriptedTunnelIo, filter_one, test_record, wait_until_send_fails,
};
use super::super::tunnel::PortTunnel;
use super::*;

struct PendingReadBrokenWrite;

impl AsyncRead for PendingReadBrokenWrite {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }
}

impl AsyncWrite for PendingReadBrokenWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        _buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "forced closed writer",
        )))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn tcp_accept_frame(stream_id: u32, listener_stream_id: u32) -> Frame {
    Frame {
        frame_type: FrameType::TcpAccept,
        flags: 0,
        stream_id,
        meta: encode_tunnel_meta(&TcpAcceptMeta {
            listener_stream_id,
            peer: "127.0.0.1:0".to_string(),
        })
        .unwrap(),
        data: Vec::new(),
    }
}

#[tokio::test]
async fn tcp_accept_send_failure_recovers_connect_tunnel_without_leaking_active_stream() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_tunnel = Arc::new(PortTunnel::from_stream(PendingReadBrokenWrite).unwrap());
    wait_until_send_fails(&connect_tunnel).await;
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();

    let control = tokio::time::timeout(
        Duration::from_secs(1),
        run_tcp_test_epoch(&runtime, listen_tunnel, connect_tunnel),
    )
    .await
    .expect("tcp epoch should finish after retryable send failure")
    .expect("retryable connect send failure should recover connect tunnel");
    assert!(matches!(
        control,
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)
    ));

    let entries = runtime
        .store
        .list(&filter_one(runtime.forward_id().as_str()))
        .await;
    assert_eq!(entries[0].active_tcp_streams, 0);
}

#[tokio::test]
async fn ready_tcp_data_send_backpressure_counts_dropped_stream() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let (connect_broker_side, mut connect_daemon_side) = tokio::io::duplex(4096);
    let connect_tunnel =
        Arc::new(PortTunnel::from_stream_with_max_queued_bytes(connect_broker_side, 1).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;

    let cancel = runtime.cancel.clone();
    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();

    let connect =
        tokio::time::timeout(Duration::from_secs(1), read_frame(&mut connect_daemon_side))
            .await
            .expect("tcp accept should open a connect stream")
            .unwrap();
    assert_eq!(connect.frame_type, FrameType::TcpConnect);
    assert_eq!(connect.stream_id, 1);

    write_frame(
        &mut connect_daemon_side,
        &Frame {
            frame_type: FrameType::TcpConnectOk,
            flags: 0,
            stream_id: 1,
            meta: Vec::new(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::TcpData,
            flags: 0,
            stream_id: 11,
            meta: Vec::new(),
            data: b"blocked".to_vec(),
        },
    )
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime
                .store
                .list(&filter_one(runtime.forward_id().as_str()))
                .await;
            if entries[0].dropped_tcp_streams == 1 {
                assert_eq!(entries[0].active_tcp_streams, 0);
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("connect data backpressure should drop the tcp stream");

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stop after cancellation")
        .unwrap()
        .expect("data backpressure should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

#[tokio::test]
async fn pending_tcp_data_limit_drops_unready_stream_without_leaking_active_count() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
    let limits = ForwardLimits {
        max_pending_tcp_bytes_per_stream: 4,
        max_pending_tcp_bytes_per_forward: 4,
        ..ForwardLimits::default()
    };
    let runtime =
        tcp_test_runtime_with_limits(listen_tunnel.clone(), connect_tunnel.clone(), limits);
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;
    let cancel = runtime.cancel.clone();

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpConnect, 1)
        .await;

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::TcpData,
            flags: 0,
            stream_id: 11,
            meta: Vec::new(),
            data: b"overflow".to_vec(),
        },
    )
    .await
    .unwrap();
    connect_io.wait_for_written_frame(FrameType::Close, 1).await;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime
                .store
                .list(&filter_one(runtime.forward_id().as_str()))
                .await;
            if entries[0].dropped_tcp_streams == 1 {
                assert_eq!(entries[0].active_tcp_streams, 0);
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("pending tcp limit should drop the unready stream");

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stop after cancellation")
        .unwrap()
        .expect("pending limit drop should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

#[tokio::test]
async fn ready_tcp_data_send_failure_recovers_connect_tunnel() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());

    let listen_session = Arc::new(ListenSessionControl::new_for_test(
        SideHandle::local().unwrap(),
        ForwardId::new("fwd_test"),
        "test-session".to_string(),
        PublicForwardPortProtocol::Tcp,
        Duration::from_secs(30),
        PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
        Some(listen_tunnel.clone()),
    ));
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime::new(
        ForwardIdentity::new(
            ForwardId::new("fwd_test"),
            SideHandle::local().unwrap(),
            SideHandle::local().unwrap(),
            PublicForwardPortProtocol::Tcp,
            "127.0.0.1:1".to_string(),
        ),
        ForwardLimits::default(),
        Default::default(),
        listen_session,
        tcp_test_epoch(listen_tunnel.clone(), connect_tunnel.clone()),
        cancel,
    );

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpConnect, 1)
        .await;
    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpConnectOk,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: Vec::new(),
    });
    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpData,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: b"ready".to_vec(),
    });

    let relayed = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut listen_daemon_side))
        .await
        .expect("connect-side data should relay after TcpConnectOk")
        .unwrap();
    assert_eq!(relayed.frame_type, FrameType::TcpData);
    assert_eq!(relayed.stream_id, 11);
    assert_eq!(relayed.data, b"ready");

    connect_io.fail_writes();
    wait_until_send_fails(&connect_tunnel).await;
    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::TcpData,
            flags: 0,
            stream_id: 11,
            meta: Vec::new(),
            data: b"after-loss".to_vec(),
        },
    )
    .await
    .unwrap();

    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should finish after retryable data send failure")
        .unwrap()
        .expect("retryable data send failure should recover connect tunnel");
    assert!(matches!(
        control,
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)
    ));
}

#[tokio::test]
async fn pending_tcp_flush_send_failure_recovers_connect_tunnel() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());

    let listen_session = Arc::new(ListenSessionControl::new_for_test(
        SideHandle::local().unwrap(),
        ForwardId::new("fwd_test"),
        "test-session".to_string(),
        PublicForwardPortProtocol::Tcp,
        Duration::from_secs(30),
        PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
        Some(listen_tunnel.clone()),
    ));
    let cancel = CancellationToken::new();
    let runtime = ForwardRuntime::new(
        ForwardIdentity::new(
            ForwardId::new("fwd_test"),
            SideHandle::local().unwrap(),
            SideHandle::local().unwrap(),
            PublicForwardPortProtocol::Tcp,
            "127.0.0.1:1".to_string(),
        ),
        ForwardLimits::default(),
        Default::default(),
        listen_session,
        tcp_test_epoch(listen_tunnel.clone(), connect_tunnel.clone()),
        cancel,
    );

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpConnect, 1)
        .await;

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::TcpData,
            flags: 0,
            stream_id: 11,
            meta: Vec::new(),
            data: b"pending".to_vec(),
        },
    )
    .await
    .unwrap();
    tokio::task::yield_now().await;

    connect_io.fail_writes();
    wait_until_send_fails(&connect_tunnel).await;
    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpConnectOk,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: Vec::new(),
    });

    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should finish after retryable pending flush failure")
        .unwrap()
        .expect("retryable pending flush failure should recover connect tunnel");
    assert!(matches!(
        control,
        ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)
    ));
}

#[tokio::test]
async fn listen_close_before_connect_ready_releases_active_stream() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;
    let cancel = runtime.cancel.clone();

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpConnect, 1)
        .await;

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::Close,
            flags: 0,
            stream_id: 11,
            meta: Vec::new(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();
    connect_io.wait_for_written_frame(FrameType::Close, 1).await;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime
                .store
                .list(&filter_one(runtime.forward_id().as_str()))
                .await;
            if entries[0].active_tcp_streams == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("listen close should release active stream before TcpConnectOk");

    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpConnectOk,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: Vec::new(),
    });
    tokio::task::yield_now().await;
    let entries = runtime.store.list(&filter_one("fwd_test")).await;
    assert_eq!(entries[0].active_tcp_streams, 0);
    assert_eq!(entries[0].dropped_tcp_streams, 0);

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stop after cancellation")
        .unwrap()
        .expect("pending close should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

#[tokio::test]
async fn fully_drained_tcp_pair_closes_both_daemon_streams() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;
    let cancel = runtime.cancel.clone();

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpConnect, 1)
        .await;
    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpConnectOk,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: Vec::new(),
    });

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::TcpEof,
            flags: 0,
            stream_id: 11,
            meta: Vec::new(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpEof, 1)
        .await;

    connect_io.push_read_frame(&Frame {
        frame_type: FrameType::TcpEof,
        flags: 0,
        stream_id: 1,
        meta: Vec::new(),
        data: Vec::new(),
    });
    connect_io.wait_for_written_frame(FrameType::Close, 1).await;
    let listen_eof =
        tokio::time::timeout(Duration::from_secs(1), read_frame(&mut listen_daemon_side))
            .await
            .expect("connect-side EOF should relay to the listen-side stream")
            .unwrap();
    assert_eq!(listen_eof.frame_type, FrameType::TcpEof);
    assert_eq!(listen_eof.stream_id, 11);
    let listen_close =
        tokio::time::timeout(Duration::from_secs(1), read_frame(&mut listen_daemon_side))
            .await
            .expect("fully drained listen-side stream should be closed after EOF relay")
            .unwrap();
    assert_eq!(listen_close.frame_type, FrameType::Close);
    assert_eq!(listen_close.stream_id, 11);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime
                .store
                .list(&filter_one(runtime.forward_id().as_str()))
                .await;
            if entries[0].active_tcp_streams == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fully drained pair should release active stream accounting");

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stop after cancellation")
        .unwrap()
        .expect("fully drained close should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

#[tokio::test]
async fn tcp_listener_error_fails_forward() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: 1,
            meta: serde_json::to_vec(&serde_json::json!({
                "code": "port_accept_failed",
                "message": "accept loop stopped",
                "fatal": false
            }))
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        run_tcp_test_epoch(&runtime, listen_tunnel, connect_tunnel),
    )
    .await
    .expect("tcp epoch should finish after listener error");
    let err = match result {
        Ok(_) => panic!("listener stream error should fail the forward"),
        Err(err) => err,
    };
    assert_eq!(
        format!("{err:#}"),
        "listen-side tcp tunnel error: port_accept_failed: accept loop stopped"
    );
}

#[tokio::test]
async fn tcp_listener_pressure_error_counts_drop_without_failing_forward() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;
    let cancel = runtime.cancel.clone();

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: 1,
            meta: serde_json::to_vec(&serde_json::json!({
                "code": RpcErrorCode::PortTunnelLimitExceeded.wire_value(),
                "message": "port tunnel active tcp stream limit reached",
                "fatal": false
            }))
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime
                .store
                .list(&filter_one(runtime.forward_id().as_str()))
                .await;
            if entries[0].dropped_tcp_streams == 1 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("listener pressure error should count a dropped tcp stream");

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stay alive after pressure error")
        .unwrap()
        .expect("pressure error should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

#[tokio::test]
async fn tcp_listener_forward_drop_counts_drop_without_failing_forward() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    runtime
        .store
        .insert(test_record(&runtime, "127.0.0.1:10000"))
        .await;
    let cancel = runtime.cancel.clone();

    let epoch_runtime = runtime.clone();
    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::ForwardDrop,
            flags: 0,
            stream_id: 1,
            meta: serde_json::to_vec(&ForwardDropMeta::new(
                ForwardDropKind::TcpStream,
                2,
                RpcErrorCode::PortTunnelLimitExceeded.wire_value(),
                Some("port tunnel active tcp stream limit reached".to_string()),
            ))
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let entries = runtime
                .store
                .list(&filter_one(runtime.forward_id().as_str()))
                .await;
            if entries[0].dropped_tcp_streams == 2 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("listener drop telemetry should count dropped tcp streams");

    cancel.cancel();
    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should stay alive after drop telemetry")
        .unwrap()
        .expect("drop telemetry should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

#[tokio::test]
async fn tcp_accepted_stream_error_closes_pair_without_failing_forward() {
    let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
    let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
    let connect_io = ScriptedTunnelIo::default();
    let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());
    let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
    let cancel = runtime.cancel.clone();

    let epoch = tokio::spawn({
        let listen_tunnel = listen_tunnel.clone();
        let connect_tunnel = connect_tunnel.clone();
        async move { run_tcp_test_epoch(&runtime, listen_tunnel, connect_tunnel).await }
    });

    write_frame(&mut listen_daemon_side, &tcp_accept_frame(11, 1))
        .await
        .unwrap();
    connect_io
        .wait_for_written_frame(FrameType::TcpConnect, 1)
        .await;

    write_frame(
        &mut listen_daemon_side,
        &Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: 11,
            meta: serde_json::to_vec(&serde_json::json!({
                "code": "port_read_failed",
                "message": "accepted stream read failed",
                "fatal": false
            }))
            .unwrap(),
            data: Vec::new(),
        },
    )
    .await
    .unwrap();
    connect_io.wait_for_written_frame(FrameType::Close, 1).await;

    cancel.cancel();

    let control = tokio::time::timeout(Duration::from_secs(1), epoch)
        .await
        .expect("tcp epoch should remain running until cancellation")
        .unwrap()
        .expect("accepted stream error should not fail the forward");
    assert!(matches!(control, ForwardLoopControl::Cancelled));
}

fn tcp_test_runtime(
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> ForwardRuntime {
    tcp_test_runtime_with_limits(listen_tunnel, connect_tunnel, ForwardLimits::default())
}

fn tcp_test_runtime_with_limits(
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
    limits: ForwardLimits,
) -> ForwardRuntime {
    let initial_epoch = tcp_test_epoch(listen_tunnel.clone(), connect_tunnel.clone());
    let listen_session = Arc::new(ListenSessionControl::new_for_test(
        SideHandle::local().unwrap(),
        ForwardId::new("fwd_test"),
        "test-session".to_string(),
        PublicForwardPortProtocol::Tcp,
        Duration::from_secs(30),
        PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
        Some(listen_tunnel),
    ));
    ForwardRuntime::new(
        ForwardIdentity::new(
            ForwardId::new("fwd_test"),
            SideHandle::local().unwrap(),
            SideHandle::local().unwrap(),
            PublicForwardPortProtocol::Tcp,
            "127.0.0.1:1".to_string(),
        ),
        limits,
        Default::default(),
        listen_session,
        initial_epoch,
        CancellationToken::new(),
    )
}

fn tcp_test_epoch(listen_tunnel: Arc<PortTunnel>, connect_tunnel: Arc<PortTunnel>) -> ForwardEpoch {
    ForwardEpoch::new(INITIAL_FORWARD_GENERATION, listen_tunnel, connect_tunnel)
}

async fn run_tcp_test_epoch(
    runtime: &ForwardRuntime,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> anyhow::Result<ForwardLoopControl> {
    let epoch = tcp_test_epoch(listen_tunnel, connect_tunnel);
    run_tcp_forward_epoch(runtime, &epoch).await
}
