use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use remote_exec_proto::port_tunnel::{Frame, FrameType};

use super::events::{ForwardLoopControl, ForwardSideEvent, TunnelRole, classify_transport_failure};
use super::supervisor::{ForwardRuntime, open_connect_tunnel, reconnect_listen_tunnel};
use super::tunnel::{
    EndpointMeta, PortTunnel, TcpAcceptMeta, classify_recoverable_tunnel_event, decode_tunnel_meta,
    encode_tunnel_meta, format_terminal_tunnel_error, is_retryable_transport_error,
};
use super::{MAX_PENDING_TCP_BYTES_PER_FORWARD, MAX_PENDING_TCP_BYTES_PER_STREAM};

struct TcpConnectStream {
    listen_stream_id: u32,
    ready: bool,
    pending_frames: Vec<Frame>,
    pending_bytes: usize,
}

#[derive(Default)]
struct PendingTcpBudget {
    total_bytes: usize,
}

pub(super) async fn run_tcp_forward(runtime: ForwardRuntime) -> anyhow::Result<()> {
    let mut listen_tunnel = runtime
        .listen_session
        .current_tunnel()
        .await
        .context("missing listen-side port tunnel")?;
    let mut connect_tunnel = runtime.initial_connect_tunnel.clone();

    loop {
        match run_tcp_forward_epoch(&runtime, listen_tunnel.clone(), connect_tunnel.clone()).await?
        {
            ForwardLoopControl::Cancelled => return Ok(()),
            ForwardLoopControl::RecoverTunnel(TunnelRole::Listen) => {
                let Some(resumed_tunnel) =
                    reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone())
                        .await?
                else {
                    return Ok(());
                };
                listen_tunnel = resumed_tunnel;
                connect_tunnel = open_connect_tunnel(&runtime.connect_side)
                    .await
                    .with_context(|| {
                        format!(
                            "reopening port tunnel to `{}` after listen-side reconnect",
                            runtime.connect_side.name()
                        )
                    })?;
            }
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
                connect_tunnel =
                    reopen_connect_tunnel(&runtime, "after connect-side reconnect").await?;
            }
        }
    }
}

async fn reopen_connect_tunnel(
    runtime: &ForwardRuntime,
    reason: &str,
) -> anyhow::Result<Arc<PortTunnel>> {
    open_connect_tunnel(&runtime.connect_side)
        .await
        .with_context(|| {
            format!(
                "reopening port tunnel to `{}` {reason}",
                runtime.connect_side.name()
            )
        })
}

async fn run_tcp_forward_epoch(
    runtime: &ForwardRuntime,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> anyhow::Result<ForwardLoopControl> {
    let mut listen_to_connect = HashMap::<u32, u32>::new();
    let mut connect_streams = HashMap::<u32, TcpConnectStream>::new();
    let mut pending_budget = PendingTcpBudget::default();
    let mut next_connect_stream_id = 1u32;

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            frame = listen_tunnel.recv() => {
                let frame = match classify_recoverable_tunnel_event(frame) {
                    ForwardSideEvent::Frame(frame) => frame,
                    ForwardSideEvent::RetryableTransportLoss => {
                        return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Listen));
                    }
                    ForwardSideEvent::TerminalTransportError(err) => {
                        return Err(err).context("reading tcp listen tunnel");
                    }
                    ForwardSideEvent::TerminalTunnelError(meta) => {
                        return Err(format_terminal_tunnel_error(&meta))
                            .context("listen-side tcp tunnel error");
                    }
                };
                match frame.frame_type {
                    FrameType::TcpAccept => {
                        let accept: TcpAcceptMeta = decode_tunnel_meta(&frame)?;
                        let connect_stream_id = next_connect_stream_id;
                        next_connect_stream_id = next_connect_stream_id.checked_add(2).unwrap_or(1);
                        if let Err(err) = connect_tunnel.send(Frame {
                            frame_type: FrameType::TcpConnect,
                            flags: 0,
                            stream_id: connect_stream_id,
                            meta: encode_tunnel_meta(&EndpointMeta {
                                endpoint: runtime.connect_endpoint.clone(),
                            })?,
                            data: Vec::new(),
                        }).await {
                            if is_retryable_transport_error(&err) {
                                if let Err(close_err) = listen_tunnel.close_stream(frame.stream_id).await {
                                    return classify_transport_failure(
                                        close_err,
                                        "closing tcp listen stream after connect tunnel loss",
                                        TunnelRole::Listen,
                                    );
                                }
                                return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect));
                            }
                            return Err(err).context("connecting tcp forward destination");
                        }
                        listen_to_connect.insert(frame.stream_id, connect_stream_id);
                        connect_streams.insert(connect_stream_id, TcpConnectStream {
                            listen_stream_id: frame.stream_id,
                            ready: false,
                            pending_frames: Vec::new(),
                            pending_bytes: 0,
                        });
                        tracing::debug!(
                            forward_id = %runtime.forward_id,
                            listener_stream_id = accept.listener_stream_id,
                            accepted_stream_id = frame.stream_id,
                            connect_stream_id,
                            "paired tcp tunnel streams"
                        );
                    }
                    FrameType::TcpData => {
                        if let Some(connect_stream_id) = listen_to_connect.get(&frame.stream_id).copied() {
                            let remapped = Frame {
                                stream_id: connect_stream_id,
                                ..frame
                            };
                            queue_or_send_tcp_connect_frame(
                                &connect_tunnel,
                                &listen_tunnel,
                                &mut listen_to_connect,
                                &mut connect_streams,
                                &mut pending_budget,
                                connect_stream_id,
                                remapped,
                            ).await?;
                        }
                    }
                    FrameType::TcpEof => {
                        if let Some(connect_stream_id) = listen_to_connect.get(&frame.stream_id).copied() {
                            let eof = Frame {
                                frame_type: frame.frame_type,
                                flags: 0,
                                stream_id: connect_stream_id,
                                meta: Vec::new(),
                                data: Vec::new(),
                            };
                            queue_or_send_tcp_connect_frame(
                                &connect_tunnel,
                                &listen_tunnel,
                                &mut listen_to_connect,
                                &mut connect_streams,
                                &mut pending_budget,
                                connect_stream_id,
                                eof,
                            ).await?;
                        }
                    }
                    FrameType::Close => {
                        if let Some(connect_stream_id) = listen_to_connect.remove(&frame.stream_id) {
                            if let Some(stream) = connect_streams.get_mut(&connect_stream_id) {
                                if stream.ready {
                                    let _ = connect_tunnel.close_stream(connect_stream_id).await;
                                    connect_streams.remove(&connect_stream_id);
                                } else {
                                    queue_or_send_tcp_connect_frame(
                                        &connect_tunnel,
                                        &listen_tunnel,
                                        &mut listen_to_connect,
                                        &mut connect_streams,
                                        &mut pending_budget,
                                        connect_stream_id,
                                        Frame {
                                            frame_type: FrameType::Close,
                                            flags: 0,
                                            stream_id: connect_stream_id,
                                            meta: Vec::new(),
                                            data: Vec::new(),
                                        },
                                    ).await?;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            frame = connect_tunnel.recv() => {
                let frame = match classify_recoverable_tunnel_event(frame) {
                    ForwardSideEvent::Frame(frame) => frame,
                    ForwardSideEvent::RetryableTransportLoss => {
                        close_active_tcp_listen_streams(
                            &listen_tunnel,
                            &mut listen_to_connect,
                            &mut connect_streams,
                            &mut pending_budget,
                        )
                        .await?;
                        return Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect));
                    }
                    ForwardSideEvent::TerminalTransportError(err) => {
                        return Err(err).context("reading tcp connect tunnel");
                    }
                    ForwardSideEvent::TerminalTunnelError(meta) => {
                        return Err(format_terminal_tunnel_error(&meta))
                            .context("connecting tcp forward destination");
                    }
                };
                match frame.frame_type {
                    FrameType::TcpConnectOk => {
                        let Some(stream) = connect_streams.get_mut(&frame.stream_id) else {
                            continue;
                        };
                        stream.ready = true;
                        let mut pending = Vec::new();
                        std::mem::swap(&mut pending, &mut stream.pending_frames);
                        let should_remove = flush_pending_tcp_connect_frames(
                            &connect_tunnel,
                            &mut listen_to_connect,
                            &mut connect_streams,
                            &mut pending_budget,
                            frame.stream_id,
                            pending,
                        ).await?;
                        if should_remove {
                            connect_streams.remove(&frame.stream_id);
                        }
                    }
                    FrameType::Error => {
                        close_tcp_pair_after_connect_error(
                            &listen_tunnel,
                            &mut listen_to_connect,
                            &mut connect_streams,
                            &mut pending_budget,
                            frame.stream_id,
                        )
                        .await?;
                    }
                    FrameType::TcpData => {
                        if let Some(listen_stream_id) = connect_streams
                            .get(&frame.stream_id)
                            .map(|stream| stream.listen_stream_id)
                        {
                            if let Err(err) = listen_tunnel.send(Frame {
                                stream_id: listen_stream_id,
                                ..frame
                            }).await {
                                return classify_transport_failure(
                                    err,
                                    "relaying tcp data to listen tunnel",
                                    TunnelRole::Listen,
                                );
                            }
                        }
                    }
                    FrameType::TcpEof => {
                        if let Some(listen_stream_id) = connect_streams
                            .get(&frame.stream_id)
                            .map(|stream| stream.listen_stream_id)
                        {
                            if let Err(err) = listen_tunnel.send(Frame {
                                frame_type: frame.frame_type,
                                flags: 0,
                                stream_id: listen_stream_id,
                                meta: Vec::new(),
                                data: Vec::new(),
                            }).await {
                                return classify_transport_failure(
                                    err,
                                    "relaying tcp eof to listen tunnel",
                                    TunnelRole::Listen,
                                );
                            }
                        }
                    }
                    FrameType::Close => {
                        if let Some(listen_stream_id) = connect_streams
                            .remove(&frame.stream_id)
                            .map(|mut stream| {
                                release_pending_budget(&mut pending_budget, &mut stream);
                                stream.listen_stream_id
                            })
                        {
                            listen_to_connect.remove(&listen_stream_id);
                            if let Err(err) = listen_tunnel.close_stream(listen_stream_id).await {
                                return classify_transport_failure(
                                    err,
                                    "closing tcp listen stream",
                                    TunnelRole::Listen,
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

async fn close_active_tcp_listen_streams(
    listen_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
) -> anyhow::Result<()> {
    let streams = std::mem::take(connect_streams);
    listen_to_connect.clear();
    for (_, mut stream) in streams {
        release_pending_budget(pending_budget, &mut stream);
        if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
            return classify_transport_failure(
                err,
                "closing tcp listen stream after connect tunnel loss",
                TunnelRole::Listen,
            )
            .map(|_| ());
        }
    }
    Ok(())
}

async fn queue_or_send_tcp_connect_frame(
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
    connect_stream_id: u32,
    frame: Frame,
) -> anyhow::Result<()> {
    let Some(stream) = connect_streams.get_mut(&connect_stream_id) else {
        return Ok(());
    };
    if stream.ready {
        if let Err(err) = connect_tunnel
            .send(frame)
            .await
            .context("relaying tcp data to connect tunnel")
        {
            if is_retryable_transport_error(&err) {
                return Ok(());
            }
            return Err(err);
        }
    } else {
        let added = frame_data_bytes(&frame);
        let next_stream_total = stream.pending_bytes.saturating_add(added);
        let next_forward_total = pending_budget.total_bytes.saturating_add(added);
        if next_stream_total > MAX_PENDING_TCP_BYTES_PER_STREAM
            || next_forward_total > MAX_PENDING_TCP_BYTES_PER_FORWARD
        {
            let listen_stream_id = stream.listen_stream_id;
            release_pending_budget(pending_budget, stream);
            connect_streams.remove(&connect_stream_id);
            listen_to_connect.remove(&listen_stream_id);
            let _ = connect_tunnel.close_stream(connect_stream_id).await;
            let _ = listen_tunnel.close_stream(listen_stream_id).await;
            return Ok(());
        }
        stream.pending_bytes = next_stream_total;
        pending_budget.total_bytes = next_forward_total;
        stream.pending_frames.push(frame);
    }
    Ok(())
}

async fn flush_pending_tcp_connect_frames(
    connect_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
    connect_stream_id: u32,
    pending_frames: Vec<Frame>,
) -> anyhow::Result<bool> {
    if let Some(stream) = connect_streams.get_mut(&connect_stream_id) {
        release_pending_budget(pending_budget, stream);
    }
    let mut should_remove = false;
    for frame in pending_frames {
        let is_close = frame.frame_type == FrameType::Close;
        if let Err(err) = connect_tunnel
            .send(frame)
            .await
            .context("relaying tcp data to connect tunnel")
        {
            if is_retryable_transport_error(&err) {
                return Ok(false);
            }
            return Err(err);
        }
        if is_close {
            if let Some(listen_stream_id) = connect_streams
                .get(&connect_stream_id)
                .map(|stream| stream.listen_stream_id)
            {
                listen_to_connect.remove(&listen_stream_id);
            }
            should_remove = true;
        }
    }
    Ok(should_remove)
}

async fn close_tcp_pair_after_connect_error(
    listen_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
    connect_stream_id: u32,
) -> anyhow::Result<()> {
    let Some(mut stream) = connect_streams.remove(&connect_stream_id) else {
        return Ok(());
    };
    release_pending_budget(pending_budget, &mut stream);
    listen_to_connect.remove(&stream.listen_stream_id);
    if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
        return classify_transport_failure(
            err,
            "closing tcp listen stream after connect error",
            TunnelRole::Listen,
        )
        .map(|_| ());
    }
    Ok(())
}

fn frame_data_bytes(frame: &Frame) -> usize {
    frame.meta.len().saturating_add(frame.data.len())
}

fn release_pending_budget(pending_budget: &mut PendingTcpBudget, stream: &mut TcpConnectStream) {
    pending_budget.total_bytes = pending_budget
        .total_bytes
        .saturating_sub(stream.pending_bytes);
    stream.pending_bytes = 0;
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context as TaskContext, Poll};
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, write_frame};
    use remote_exec_proto::public::ForwardPortProtocol as PublicForwardPortProtocol;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use super::super::side::SideHandle;
    use super::super::supervisor::{ForwardRuntime, ListenSessionControl};
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

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn tcp_accept_send_failure_recovers_connect_tunnel_and_closes_listen_stream() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_tunnel = Arc::new(PortTunnel::from_stream(PendingReadBrokenWrite).unwrap());
        wait_until_send_fails(&connect_tunnel).await;

        let listen_session = Arc::new(ListenSessionControl {
            side: SideHandle::local(),
            session_id: "test-session".to_string(),
            listener_stream_id: 1,
            resume_timeout: Duration::from_secs(30),
            current_tunnel: Mutex::new(Some(listen_tunnel.clone())),
            op_lock: Mutex::new(()),
        });
        let cancel = CancellationToken::new();
        let runtime = ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local(),
            connect_side: SideHandle::local(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            listen_session,
            initial_connect_tunnel: connect_tunnel.clone(),
            cancel,
        };

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::TcpAccept,
                flags: 0,
                stream_id: 11,
                meta: serde_json::to_vec(&serde_json::json!({
                    "listener_stream_id": 1
                }))
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        let control = tokio::time::timeout(
            Duration::from_secs(1),
            run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel),
        )
        .await
        .expect("tcp epoch should finish after retryable send failure")
        .expect("retryable connect send failure should recover connect tunnel");
        assert!(matches!(
            control,
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)
        ));

        let close =
            tokio::time::timeout(Duration::from_secs(1), read_frame(&mut listen_daemon_side))
                .await
                .expect("listen stream should be closed after failed connect send")
                .unwrap();
        assert_eq!(close.frame_type, FrameType::Close);
        assert_eq!(close.stream_id, 11);
    }

    async fn wait_until_send_fails(tunnel: &PortTunnel) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let result = tunnel
                    .send(Frame {
                        frame_type: FrameType::Close,
                        flags: 0,
                        stream_id: 99,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                if result.is_err() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("connect tunnel writer should close after forced write failure");
    }
}
