use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use remote_exec_proto::port_tunnel::{Frame, FrameType};
use remote_exec_proto::public::ForwardPortSideRole;

use super::events::{ForwardLoopControl, ForwardSideEvent, TunnelRole, classify_transport_failure};
use super::supervisor::{ForwardRuntime, open_connect_tunnel, reconnect_listen_tunnel};
use super::tunnel::{
    EndpointMeta, PortTunnel, TcpAcceptMeta, classify_recoverable_tunnel_event,
    decode_tunnel_error_frame, decode_tunnel_meta, encode_tunnel_meta,
    format_terminal_tunnel_error, is_retryable_transport_error,
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
                runtime
                    .store
                    .mark_reconnecting(
                        &runtime.forward_id,
                        ForwardPortSideRole::Listen,
                        "listen-side tunnel lost".to_string(),
                    )
                    .await;
                let Some(resumed_tunnel) =
                    reconnect_listen_tunnel(runtime.listen_session.clone(), runtime.cancel.clone())
                        .await?
                else {
                    return Ok(());
                };
                listen_tunnel = resumed_tunnel;
                runtime
                    .store
                    .mark_ready(&runtime.forward_id, ForwardPortSideRole::Listen)
                    .await;
                connect_tunnel = open_connect_tunnel(&runtime.connect_side)
                    .await
                    .with_context(|| {
                        format!(
                            "reopening port tunnel to `{}` after listen-side reconnect",
                            runtime.connect_side.name()
                        )
                    })?;
                runtime
                    .store
                    .mark_ready(&runtime.forward_id, ForwardPortSideRole::Connect)
                    .await;
            }
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
                runtime
                    .store
                    .mark_reconnecting(
                        &runtime.forward_id,
                        ForwardPortSideRole::Connect,
                        "connect-side tunnel lost".to_string(),
                    )
                    .await;
                connect_tunnel =
                    reopen_connect_tunnel(&runtime, "after connect-side reconnect").await?;
                runtime
                    .store
                    .mark_ready(&runtime.forward_id, ForwardPortSideRole::Connect)
                    .await;
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
                            if let Some(control) = queue_or_send_tcp_connect_frame(
                                runtime,
                                &connect_tunnel,
                                &listen_tunnel,
                                &mut listen_to_connect,
                                &mut connect_streams,
                                &mut pending_budget,
                                connect_stream_id,
                                remapped,
                            ).await? {
                                return Ok(control);
                            }
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
                            if let Some(control) = queue_or_send_tcp_connect_frame(
                                runtime,
                                &connect_tunnel,
                                &listen_tunnel,
                                &mut listen_to_connect,
                                &mut connect_streams,
                                &mut pending_budget,
                                connect_stream_id,
                                eof,
                            ).await? {
                                return Ok(control);
                            }
                        }
                    }
                    FrameType::Close => {
                        if let Some(connect_stream_id) = listen_to_connect.remove(&frame.stream_id) {
                            if let Some(stream) = connect_streams.get_mut(&connect_stream_id) {
                                if stream.ready {
                                    let _ = connect_tunnel.close_stream(connect_stream_id).await;
                                    connect_streams.remove(&connect_stream_id);
                                } else {
                                    if let Some(control) = queue_or_send_tcp_connect_frame(
                                        runtime,
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
                                    ).await? {
                                        return Ok(control);
                                    }
                                }
                            }
                        }
                    }
                    FrameType::Error => {
                        if frame.stream_id == runtime.listen_session.listener_stream_id {
                            return Err(format_terminal_tunnel_error(
                                &decode_tunnel_error_frame(&frame),
                            ))
                            .context("listen-side tcp tunnel error");
                        }
                        if let Some(connect_stream_id) = listen_to_connect.remove(&frame.stream_id) {
                            if let Some(mut stream) = connect_streams.remove(&connect_stream_id) {
                                release_pending_budget(&mut pending_budget, &mut stream);
                            }
                            if let Err(err) = connect_tunnel.close_stream(connect_stream_id).await {
                                return classify_transport_failure(
                                    err,
                                    "closing tcp connect stream after listen error",
                                    TunnelRole::Connect,
                                );
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
                            runtime,
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
                        let flush_result = flush_pending_tcp_connect_frames(
                            runtime,
                            &connect_tunnel,
                            &listen_tunnel,
                            &mut listen_to_connect,
                            &mut connect_streams,
                            &mut pending_budget,
                            frame.stream_id,
                            pending,
                        ).await?;
                        match flush_result {
                            TcpFlushResult::Sent { should_remove } => {
                                if should_remove {
                                    connect_streams.remove(&frame.stream_id);
                                }
                            }
                            TcpFlushResult::Recover(control) => return Ok(control),
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
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
) -> anyhow::Result<()> {
    let streams = std::mem::take(connect_streams);
    let dropped_count = streams.len() as u64;
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
    if dropped_count > 0 {
        runtime
            .store
            .update_entry(&runtime.forward_id, |entry| {
                entry.dropped_tcp_streams += dropped_count;
                entry.active_tcp_streams = entry.active_tcp_streams.saturating_sub(dropped_count);
            })
            .await;
    }
    Ok(())
}

async fn queue_or_send_tcp_connect_frame(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
    connect_stream_id: u32,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(stream_ready) = connect_streams
        .get(&connect_stream_id)
        .map(|stream| stream.ready)
    else {
        return Ok(None);
    };
    if stream_ready {
        if let Err(err) = connect_tunnel
            .send(frame)
            .await
            .context("relaying tcp data to connect tunnel")
        {
            if is_retryable_transport_error(&err) {
                close_active_tcp_listen_streams(
                    runtime,
                    listen_tunnel,
                    listen_to_connect,
                    connect_streams,
                    pending_budget,
                )
                .await?;
                return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
            }
            return Err(err);
        }
    } else {
        let Some(stream) = connect_streams.get_mut(&connect_stream_id) else {
            return Ok(None);
        };
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
            return Ok(None);
        }
        stream.pending_bytes = next_stream_total;
        pending_budget.total_bytes = next_forward_total;
        stream.pending_frames.push(frame);
    }
    Ok(None)
}

async fn flush_pending_tcp_connect_frames(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    listen_to_connect: &mut HashMap<u32, u32>,
    connect_streams: &mut HashMap<u32, TcpConnectStream>,
    pending_budget: &mut PendingTcpBudget,
    connect_stream_id: u32,
    pending_frames: Vec<Frame>,
) -> anyhow::Result<TcpFlushResult> {
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
                close_active_tcp_listen_streams(
                    runtime,
                    listen_tunnel,
                    listen_to_connect,
                    connect_streams,
                    pending_budget,
                )
                .await?;
                return Ok(TcpFlushResult::Recover(ForwardLoopControl::RecoverTunnel(
                    TunnelRole::Connect,
                )));
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
    Ok(TcpFlushResult::Sent { should_remove })
}

enum TcpFlushResult {
    Sent { should_remove: bool },
    Recover(ForwardLoopControl),
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
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::task::{Context as TaskContext, Poll, Waker};
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{Frame, FrameType, HEADER_LEN, read_frame, write_frame};
    use remote_exec_proto::public::ForwardPortProtocol as PublicForwardPortProtocol;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::sync::Mutex as TokioMutex;
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

    #[derive(Clone, Default)]
    struct ScriptedTunnelIo {
        state: Arc<StdMutex<ScriptedTunnelState>>,
    }

    #[derive(Default)]
    struct ScriptedTunnelState {
        read_bytes: VecDeque<u8>,
        written_bytes: Vec<u8>,
        fail_writes: bool,
        read_waker: Option<Waker>,
    }

    impl ScriptedTunnelIo {
        fn fail_writes(&self) {
            self.state.lock().unwrap().fail_writes = true;
        }

        fn push_read_frame(&self, frame: &Frame) {
            let mut state = self.state.lock().unwrap();
            state.read_bytes.extend(frame_bytes(frame));
            if let Some(waker) = state.read_waker.take() {
                waker.wake();
            }
        }

        async fn wait_for_written_frame(&self, frame_type: FrameType, stream_id: u32) {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if self.pop_matching_written_frame(frame_type, stream_id) {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("expected tunnel frame should be written");
        }

        fn pop_matching_written_frame(&self, frame_type: FrameType, stream_id: u32) -> bool {
            let mut state = self.state.lock().unwrap();
            if state.written_bytes.len() < HEADER_LEN {
                return false;
            }
            let meta_len = u32::from_be_bytes(
                state.written_bytes[8..12]
                    .try_into()
                    .expect("header slice length"),
            ) as usize;
            let data_len = u32::from_be_bytes(
                state.written_bytes[12..16]
                    .try_into()
                    .expect("header slice length"),
            ) as usize;
            let frame_len = HEADER_LEN + meta_len + data_len;
            if state.written_bytes.len() < frame_len {
                return false;
            }
            let written_frame_type = state.written_bytes[0];
            let written_stream_id = u32::from_be_bytes(
                state.written_bytes[4..8]
                    .try_into()
                    .expect("header slice length"),
            );
            assert_eq!(written_frame_type, frame_type as u8);
            assert_eq!(written_stream_id, stream_id);
            state.written_bytes.drain(..frame_len);
            true
        }
    }

    impl AsyncRead for ScriptedTunnelIo {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut TaskContext<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let mut state = self.state.lock().unwrap();
            if state.read_bytes.is_empty() {
                state.read_waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let read = buf.remaining().min(state.read_bytes.len());
            let bytes: Vec<u8> = state.read_bytes.drain(..read).collect();
            buf.put_slice(&bytes);
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for ScriptedTunnelIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            let mut state = self.state.lock().unwrap();
            if state.fail_writes {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "forced closed writer",
                )));
            }
            state.written_bytes.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
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
            forward_id: "fwd_test".to_string(),
            session_id: "test-session".to_string(),
            generation: 1,
            listener_stream_id: 1,
            resume_timeout: Duration::from_secs(30),
            current_tunnel: TokioMutex::new(Some(listen_tunnel.clone())),
            op_lock: TokioMutex::new(()),
        });
        let cancel = CancellationToken::new();
        let runtime = ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local(),
            connect_side: SideHandle::local(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            store: Default::default(),
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

    #[tokio::test]
    async fn ready_tcp_data_send_failure_recovers_connect_tunnel() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());

        let listen_session = Arc::new(ListenSessionControl {
            side: SideHandle::local(),
            forward_id: "fwd_test".to_string(),
            session_id: "test-session".to_string(),
            generation: 1,
            listener_stream_id: 1,
            resume_timeout: Duration::from_secs(30),
            current_tunnel: TokioMutex::new(Some(listen_tunnel.clone())),
            op_lock: TokioMutex::new(()),
        });
        let cancel = CancellationToken::new();
        let runtime = ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local(),
            connect_side: SideHandle::local(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel.clone(),
            cancel,
        };

        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel).await }
        });

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

        let relayed =
            tokio::time::timeout(Duration::from_secs(1), read_frame(&mut listen_daemon_side))
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

        let listen_session = Arc::new(ListenSessionControl {
            side: SideHandle::local(),
            forward_id: "fwd_test".to_string(),
            session_id: "test-session".to_string(),
            generation: 1,
            listener_stream_id: 1,
            resume_timeout: Duration::from_secs(30),
            current_tunnel: TokioMutex::new(Some(listen_tunnel.clone())),
            op_lock: TokioMutex::new(()),
        });
        let cancel = CancellationToken::new();
        let runtime = ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local(),
            connect_side: SideHandle::local(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel.clone(),
            cancel,
        };

        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel).await }
        });

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
            run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel),
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
            async move { run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel).await }
        });

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
        let listen_session = Arc::new(ListenSessionControl {
            side: SideHandle::local(),
            forward_id: "fwd_test".to_string(),
            session_id: "test-session".to_string(),
            generation: 1,
            listener_stream_id: 1,
            resume_timeout: Duration::from_secs(30),
            current_tunnel: TokioMutex::new(Some(listen_tunnel)),
            op_lock: TokioMutex::new(()),
        });
        ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local(),
            connect_side: SideHandle::local(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel,
            cancel: CancellationToken::new(),
        }
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

    fn frame_bytes(frame: &Frame) -> Vec<u8> {
        let mut bytes = vec![0; HEADER_LEN];
        bytes[0] = frame.frame_type as u8;
        bytes[1] = frame.flags;
        bytes[4..8].copy_from_slice(&frame.stream_id.to_be_bytes());
        bytes[8..12].copy_from_slice(&(frame.meta.len() as u32).to_be_bytes());
        bytes[12..16].copy_from_slice(&(frame.data.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&frame.meta);
        bytes.extend_from_slice(&frame.data);
        bytes
    }
}
