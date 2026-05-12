use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use remote_exec_proto::port_tunnel::{EndpointMeta, Frame, FrameType, TcpAcceptMeta};

use super::apply_forward_drop_report;
use super::events::{ForwardLoopControl, ForwardSideEvent, TunnelRole, classify_transport_failure};
use super::generation::StreamIdAllocator;
use super::supervisor::{ForwardRuntime, reconnect_connect_tunnel, recover_listen_side_tunnels};
use super::tunnel::{
    PortTunnel, classify_recoverable_tunnel_event, decode_tunnel_error_frame, decode_tunnel_meta,
    encode_tunnel_meta,
    format_terminal_tunnel_error, is_backpressure_error, is_recoverable_pressure_tunnel_error,
    is_retryable_transport_error,
};

struct TcpConnectStream {
    listen_stream_id: u32,
    ready: bool,
    listen_eof: bool,
    connect_eof: bool,
    pending_frames: Vec<Frame>,
    pending_bytes: usize,
}

#[derive(Default)]
struct PendingTcpBudget {
    total_bytes: usize,
}

#[derive(Default)]
struct TcpForwardState {
    listen_to_connect: HashMap<u32, u32>,
    connect_streams: HashMap<u32, TcpConnectStream>,
    pending_budget: PendingTcpBudget,
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
                let Some(recovered) = recover_listen_side_tunnels(&runtime).await? else {
                    return Ok(());
                };
                listen_tunnel = recovered.listen_tunnel;
                connect_tunnel = recovered.connect_tunnel;
            }
            ForwardLoopControl::RecoverTunnel(TunnelRole::Connect) => {
                let Some(reconnected_tunnel) = reconnect_connect_tunnel(&runtime).await? else {
                    return Ok(());
                };
                connect_tunnel = reconnected_tunnel;
            }
        }
    }
}

async fn run_tcp_forward_epoch(
    runtime: &ForwardRuntime,
    listen_tunnel: Arc<PortTunnel>,
    connect_tunnel: Arc<PortTunnel>,
) -> anyhow::Result<ForwardLoopControl> {
    let mut state = TcpForwardState::default();
    let mut connect_stream_ids = StreamIdAllocator::new_odd();

    loop {
        tokio::select! {
            _ = runtime.cancel.cancelled() => return Ok(ForwardLoopControl::Cancelled),
            frame = listen_tunnel.recv() => {
                if let Some(control) = handle_listen_tunnel_event(
                    runtime,
                    &listen_tunnel,
                    &connect_tunnel,
                    &mut state,
                    &mut connect_stream_ids,
                    frame,
                ).await? {
                    return Ok(control);
                }
            }
            frame = connect_tunnel.recv() => {
                if let Some(control) = handle_connect_tunnel_event(
                    runtime,
                    &listen_tunnel,
                    &connect_tunnel,
                    &mut state,
                    frame,
                ).await? {
                    return Ok(control);
                }
            }
        }
    }
}

async fn handle_listen_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_ids: &mut StreamIdAllocator,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let frame = match classify_recoverable_tunnel_event(frame_result) {
        ForwardSideEvent::Frame(frame) => frame,
        ForwardSideEvent::RetryableTransportLoss => {
            return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Listen)));
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
            handle_listen_tcp_accept(
                runtime,
                listen_tunnel,
                connect_tunnel,
                state,
                connect_stream_ids,
                frame,
            )
            .await
        }
        FrameType::TcpData => {
            handle_listen_tcp_data(runtime, listen_tunnel, connect_tunnel, state, frame).await
        }
        FrameType::TcpEof => {
            handle_listen_tcp_eof(runtime, listen_tunnel, connect_tunnel, state, frame).await
        }
        FrameType::Close => {
            handle_listen_close(runtime, connect_tunnel, state, frame).await;
            Ok(None)
        }
        FrameType::Error => handle_listen_error(runtime, connect_tunnel, state, frame).await,
        FrameType::ForwardDrop => {
            apply_forward_drop_report(&runtime.store, &runtime.forward_id, &frame).await?;
            Ok(None)
        }
        _ => Ok(None),
    }
}

async fn handle_connect_tunnel_event(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame_result: anyhow::Result<Frame>,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let frame = match classify_recoverable_tunnel_event(frame_result) {
        ForwardSideEvent::Frame(frame) => frame,
        ForwardSideEvent::RetryableTransportLoss => {
            close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
            return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
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
            handle_connect_tcp_connect_ok(runtime, listen_tunnel, connect_tunnel, state, frame)
                .await
        }
        FrameType::Error => {
            handle_connect_error(runtime, listen_tunnel, state, frame).await?;
            Ok(None)
        }
        FrameType::TcpData => handle_connect_tcp_data(listen_tunnel, state, frame).await,
        FrameType::TcpEof => {
            handle_connect_tcp_eof(runtime, listen_tunnel, connect_tunnel, state, frame).await
        }
        FrameType::Close => handle_connect_close(runtime, listen_tunnel, state, frame).await,
        _ => Ok(None),
    }
}

async fn handle_listen_tcp_accept(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_ids: &mut StreamIdAllocator,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let accept: TcpAcceptMeta = decode_tunnel_meta(&frame)?;
    if !try_reserve_active_tcp_stream(runtime).await {
        let _ = listen_tunnel.close_stream(frame.stream_id).await;
        runtime.record_dropped_stream().await;
        return Ok(None);
    }
    let Some(connect_stream_id) = connect_stream_ids.next() else {
        debug_assert!(connect_stream_ids.needs_generation_rotation());
        let _ = listen_tunnel.close_stream(frame.stream_id).await;
        drop_active_tcp_stream(runtime).await;
        close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
        return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
    };
    if let Err(err) = connect_tunnel
        .send(Frame {
            frame_type: FrameType::TcpConnect,
            flags: 0,
            stream_id: connect_stream_id,
            meta: encode_tunnel_meta(&EndpointMeta {
                endpoint: runtime.connect_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await
    {
        if is_retryable_transport_error(&err) {
            return close_unpaired_listen_stream_after_connect_loss(
                runtime,
                listen_tunnel,
                frame.stream_id,
            )
            .await
            .map(Some);
        }
        drop_active_tcp_stream(runtime).await;
        return Err(err).context("connecting tcp forward destination");
    }
    state
        .listen_to_connect
        .insert(frame.stream_id, connect_stream_id);
    state.connect_streams.insert(
        connect_stream_id,
        TcpConnectStream {
            listen_stream_id: frame.stream_id,
            ready: false,
            listen_eof: false,
            connect_eof: false,
            pending_frames: Vec::new(),
            pending_bytes: 0,
        },
    );
    tracing::debug!(
        forward_id = %runtime.forward_id,
        listener_stream_id = accept.listener_stream_id,
        accepted_stream_id = frame.stream_id,
        connect_stream_id,
        "paired tcp tunnel streams"
    );
    Ok(None)
}

async fn handle_listen_tcp_data(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(connect_stream_id) = state.listen_to_connect.get(&frame.stream_id).copied() else {
        return Ok(None);
    };
    queue_or_send_tcp_connect_frame(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        connect_stream_id,
        Frame {
            stream_id: connect_stream_id,
            ..frame
        },
    )
    .await
}

async fn handle_listen_tcp_eof(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(connect_stream_id) = state.listen_to_connect.get(&frame.stream_id).copied() else {
        return Ok(None);
    };
    if let Some(stream) = state.connect_streams.get_mut(&connect_stream_id) {
        stream.listen_eof = true;
    }
    if let Some(control) = queue_or_send_tcp_connect_frame(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        connect_stream_id,
        Frame {
            frame_type: frame.frame_type,
            flags: 0,
            stream_id: connect_stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        },
    )
    .await?
    {
        return Ok(Some(control));
    }
    close_tcp_pair_if_fully_eof(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        connect_stream_id,
    )
    .await
}

async fn handle_listen_close(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) {
    if let Some(connect_stream_id) = state.listen_to_connect.remove(&frame.stream_id) {
        if let Some(mut stream) = state.connect_streams.remove(&connect_stream_id) {
            if !stream.ready {
                release_pending_budget(&mut state.pending_budget, &mut stream);
            }
            let _ = connect_tunnel.close_stream(connect_stream_id).await;
            release_active_tcp_stream(runtime).await;
        }
    }
}

async fn handle_listen_error(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    if frame.stream_id == runtime.listen_session.listener_stream_id {
        let meta = decode_tunnel_error_frame(&frame);
        if is_recoverable_pressure_tunnel_error(&meta) {
            runtime.record_dropped_stream().await;
            return Ok(None);
        }
        return Err(format_terminal_tunnel_error(&meta)).context("listen-side tcp tunnel error");
    }
    if let Some(connect_stream_id) = state.listen_to_connect.remove(&frame.stream_id) {
        if let Some(mut stream) = state.connect_streams.remove(&connect_stream_id) {
            release_pending_budget(&mut state.pending_budget, &mut stream);
            release_active_tcp_stream(runtime).await;
        }
        if let Err(err) = connect_tunnel.close_stream(connect_stream_id).await {
            return classify_transport_failure(
                err,
                "closing tcp connect stream after listen error",
                TunnelRole::Connect,
            )
            .map(Some);
        }
    }
    Ok(None)
}

async fn handle_connect_tcp_connect_ok(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(stream) = state.connect_streams.get_mut(&frame.stream_id) else {
        return Ok(None);
    };
    stream.ready = true;
    let mut pending = Vec::new();
    std::mem::swap(&mut pending, &mut stream.pending_frames);
    match flush_pending_tcp_connect_frames(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        frame.stream_id,
        pending,
    )
    .await?
    {
        TcpFlushResult::Sent { should_remove } => {
            if should_remove {
                state.connect_streams.remove(&frame.stream_id);
            }
            Ok(None)
        }
        TcpFlushResult::Recover(control) => Ok(Some(control)),
    }
}

async fn handle_connect_error(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<()> {
    close_tcp_pair_after_connect_error(runtime, listen_tunnel, state, frame.stream_id).await
}

async fn handle_connect_tcp_data(
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(listen_stream_id) = state
        .connect_streams
        .get(&frame.stream_id)
        .map(|stream| stream.listen_stream_id)
    else {
        return Ok(None);
    };
    if let Err(err) = listen_tunnel
        .send(Frame {
            stream_id: listen_stream_id,
            ..frame
        })
        .await
    {
        return classify_transport_failure(
            err,
            "relaying tcp data to listen tunnel",
            TunnelRole::Listen,
        )
        .map(Some);
    }
    Ok(None)
}

async fn handle_connect_tcp_eof(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    connect_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(listen_stream_id) = state
        .connect_streams
        .get_mut(&frame.stream_id)
        .map(|stream| {
            stream.connect_eof = true;
            stream.listen_stream_id
        })
    else {
        return Ok(None);
    };
    if let Err(err) = listen_tunnel
        .send(Frame {
            frame_type: frame.frame_type,
            flags: 0,
            stream_id: listen_stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await
    {
        return classify_transport_failure(
            err,
            "relaying tcp eof to listen tunnel",
            TunnelRole::Listen,
        )
        .map(Some);
    }
    close_tcp_pair_if_fully_eof(
        runtime,
        connect_tunnel,
        listen_tunnel,
        state,
        frame.stream_id,
    )
    .await
}

async fn handle_connect_close(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    if let Some(listen_stream_id) =
        state
            .connect_streams
            .remove(&frame.stream_id)
            .map(|mut stream| {
                release_pending_budget(&mut state.pending_budget, &mut stream);
                stream.listen_stream_id
            })
    {
        state.listen_to_connect.remove(&listen_stream_id);
        release_active_tcp_stream(runtime).await;
        if let Err(err) = listen_tunnel.close_stream(listen_stream_id).await {
            return classify_transport_failure(
                err,
                "closing tcp listen stream",
                TunnelRole::Listen,
            )
            .map(Some);
        }
    }
    Ok(None)
}

async fn close_active_tcp_listen_streams(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
) -> anyhow::Result<()> {
    let streams = std::mem::take(&mut state.connect_streams);
    let dropped_count = streams.len() as u64;
    state.listen_to_connect.clear();
    let mut first_error = None;
    for (_, mut stream) in streams {
        release_pending_budget(&mut state.pending_budget, &mut stream);
        if let Err(err) = listen_tunnel.close_stream(stream.listen_stream_id).await {
            let classified = classify_transport_failure(
                err,
                "closing tcp listen stream after connect tunnel loss",
                TunnelRole::Listen,
            )
            .map(|_| ());
            if first_error.is_none() {
                first_error = Some(classified);
            }
        }
    }
    runtime
        .record_dropped_streams_and_release_active(dropped_count)
        .await;
    if let Some(result) = first_error {
        result?;
    }
    Ok(())
}

async fn close_unpaired_listen_stream_after_connect_loss(
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    listen_stream_id: u32,
) -> anyhow::Result<ForwardLoopControl> {
    let close_result = listen_tunnel.close_stream(listen_stream_id).await;
    drop_active_tcp_stream(runtime).await;
    if let Err(err) = close_result {
        return classify_transport_failure(
            err,
            "closing tcp listen stream after connect tunnel loss",
            TunnelRole::Listen,
        );
    }
    Ok(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect))
}

async fn queue_or_send_tcp_connect_frame(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    frame: Frame,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some(stream_ready) = state
        .connect_streams
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
            if is_backpressure_error(&err) {
                close_tcp_pair_after_connect_pressure(
                    runtime,
                    connect_tunnel,
                    listen_tunnel,
                    state,
                    connect_stream_id,
                )
                .await?;
                return Ok(None);
            }
            if is_retryable_transport_error(&err) {
                close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
                return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
            }
            return Err(err);
        }
    } else {
        let Some(stream) = state.connect_streams.get_mut(&connect_stream_id) else {
            return Ok(None);
        };
        let added = frame_data_bytes(&frame);
        let next_stream_total = stream.pending_bytes.saturating_add(added);
        let next_forward_total = state.pending_budget.total_bytes.saturating_add(added);
        if next_stream_total > runtime.max_pending_tcp_bytes_per_stream
            || next_forward_total > runtime.max_pending_tcp_bytes_per_forward
        {
            let listen_stream_id = stream.listen_stream_id;
            release_pending_budget(&mut state.pending_budget, stream);
            state.connect_streams.remove(&connect_stream_id);
            state.listen_to_connect.remove(&listen_stream_id);
            let _ = connect_tunnel.close_stream(connect_stream_id).await;
            let _ = listen_tunnel.close_stream(listen_stream_id).await;
            runtime.record_dropped_stream().await;
            release_active_tcp_stream(runtime).await;
            return Ok(None);
        }
        stream.pending_bytes = next_stream_total;
        state.pending_budget.total_bytes = next_forward_total;
        stream.pending_frames.push(frame);
    }
    Ok(None)
}

async fn flush_pending_tcp_connect_frames(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
    pending_frames: Vec<Frame>,
) -> anyhow::Result<TcpFlushResult> {
    if let Some(stream) = state.connect_streams.get_mut(&connect_stream_id) {
        release_pending_budget(&mut state.pending_budget, stream);
    }
    let mut should_remove = false;
    for frame in pending_frames {
        let is_close = frame.frame_type == FrameType::Close;
        if let Err(err) = connect_tunnel
            .send(frame)
            .await
            .context("relaying tcp data to connect tunnel")
        {
            if is_backpressure_error(&err) {
                close_tcp_pair_after_connect_pressure(
                    runtime,
                    connect_tunnel,
                    listen_tunnel,
                    state,
                    connect_stream_id,
                )
                .await?;
                return Ok(TcpFlushResult::Sent {
                    should_remove: true,
                });
            }
            if is_retryable_transport_error(&err) {
                close_active_tcp_listen_streams(runtime, listen_tunnel, state).await?;
                return Ok(TcpFlushResult::Recover(ForwardLoopControl::RecoverTunnel(
                    TunnelRole::Connect,
                )));
            }
            return Err(err);
        }
        if is_close {
            if let Some(listen_stream_id) = state
                .connect_streams
                .get(&connect_stream_id)
                .map(|stream| stream.listen_stream_id)
            {
                state.listen_to_connect.remove(&listen_stream_id);
            }
            release_active_tcp_stream(runtime).await;
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
    runtime: &ForwardRuntime,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
) -> anyhow::Result<()> {
    let Some(mut stream) = state.connect_streams.remove(&connect_stream_id) else {
        return Ok(());
    };
    release_pending_budget(&mut state.pending_budget, &mut stream);
    state.listen_to_connect.remove(&stream.listen_stream_id);
    release_active_tcp_stream(runtime).await;
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

async fn close_tcp_pair_after_connect_pressure(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
) -> anyhow::Result<()> {
    let Some(mut stream) = state.connect_streams.remove(&connect_stream_id) else {
        return Ok(());
    };
    release_pending_budget(&mut state.pending_budget, &mut stream);
    state.listen_to_connect.remove(&stream.listen_stream_id);
    let _ = connect_tunnel.close_stream(connect_stream_id).await;
    let _ = listen_tunnel.close_stream(stream.listen_stream_id).await;
    runtime.record_dropped_stream().await;
    release_active_tcp_stream(runtime).await;
    Ok(())
}

async fn close_tcp_pair_if_fully_eof(
    runtime: &ForwardRuntime,
    connect_tunnel: &Arc<PortTunnel>,
    listen_tunnel: &Arc<PortTunnel>,
    state: &mut TcpForwardState,
    connect_stream_id: u32,
) -> anyhow::Result<Option<ForwardLoopControl>> {
    let Some((listen_stream_id, fully_eof)) =
        state.connect_streams.get(&connect_stream_id).map(|stream| {
            (
                stream.listen_stream_id,
                stream.listen_eof && stream.connect_eof,
            )
        })
    else {
        return Ok(None);
    };
    if !fully_eof {
        return Ok(None);
    }
    if let Some(mut stream) = state.connect_streams.remove(&connect_stream_id) {
        release_pending_budget(&mut state.pending_budget, &mut stream);
    }
    state.listen_to_connect.remove(&listen_stream_id);
    if let Err(err) = connect_tunnel.close_stream(connect_stream_id).await {
        release_active_tcp_stream(runtime).await;
        if is_retryable_transport_error(&err) {
            if let Err(listen_err) = listen_tunnel.close_stream(listen_stream_id).await {
                return classify_transport_failure(
                    listen_err,
                    "closing fully drained tcp listen stream after connect tunnel loss",
                    TunnelRole::Listen,
                )
                .map(Some);
            }
            return Ok(Some(ForwardLoopControl::RecoverTunnel(TunnelRole::Connect)));
        }
        return Err(err).context("closing fully drained tcp connect stream");
    }
    if let Err(err) = listen_tunnel.close_stream(listen_stream_id).await {
        release_active_tcp_stream(runtime).await;
        return classify_transport_failure(
            err,
            "closing fully drained tcp listen stream",
            TunnelRole::Listen,
        )
        .map(Some);
    }
    release_active_tcp_stream(runtime).await;
    Ok(None)
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

async fn try_reserve_active_tcp_stream(runtime: &ForwardRuntime) -> bool {
    let mut reserved = false;
    let mut saw_entry = false;
    runtime
        .store
        .update_entry(&runtime.forward_id, |entry| {
            saw_entry = true;
            if entry.active_tcp_streams < runtime.max_active_tcp_streams_per_forward {
                entry.active_tcp_streams += 1;
                reserved = true;
            }
        })
        .await;
    !saw_entry || reserved
}

async fn release_active_tcp_stream(runtime: &ForwardRuntime) {
    runtime.release_active_stream().await;
}

async fn drop_active_tcp_stream(runtime: &ForwardRuntime) {
    runtime.record_dropped_active_stream().await;
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::task::{Context as TaskContext, Poll, Waker};
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        ForwardDropKind, ForwardDropMeta, Frame, FrameType, HEADER_LEN, read_frame, write_frame,
    };
    use remote_exec_proto::public::{
        ForwardPortEntry, ForwardPortLimitSummary, ForwardPortProtocol as PublicForwardPortProtocol,
    };
    use remote_exec_proto::rpc::RpcErrorCode;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
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

        // The failed connect tunnel and queued listen accept may be observed in
        // either order; neither path may leak active stream accounting.
        let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
        assert_eq!(entries[0].active_tcp_streams, 0);
    }

    #[tokio::test]
    async fn tcp_accept_send_backpressure_counts_dropped_stream() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let (connect_broker_side, _connect_daemon_side) = tokio::io::duplex(4096);
        let connect_tunnel = Arc::new(
            PortTunnel::from_stream_with_max_queued_bytes(connect_broker_side, 1).unwrap(),
        );
        let runtime = tcp_test_runtime(listen_tunnel.clone(), connect_tunnel.clone());
        runtime
            .store
            .insert(test_record(&runtime, "127.0.0.1:10000"))
            .await;

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

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            run_tcp_forward_epoch(&runtime, listen_tunnel, connect_tunnel),
        )
        .await
        .expect("tcp epoch should finish after immediate connect send backpressure");
        let error = match result {
            Ok(_) => panic!("connect send backpressure should fail the tcp epoch"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains("port_forward_backpressure_exceeded"),
            "unexpected error: {error:#}"
        );

        let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
        assert_eq!(entries[0].active_tcp_streams, 0);
        assert_eq!(entries[0].dropped_tcp_streams, 1);
    }

    #[tokio::test]
    async fn ready_tcp_data_send_failure_recovers_connect_tunnel() {
        let (listen_broker_side, mut listen_daemon_side) = tokio::io::duplex(4096);
        let listen_tunnel = Arc::new(PortTunnel::from_stream(listen_broker_side).unwrap());
        let connect_io = ScriptedTunnelIo::default();
        let connect_tunnel = Arc::new(PortTunnel::from_stream(connect_io.clone()).unwrap());

        let listen_session = Arc::new(ListenSessionControl::new_for_test(
            SideHandle::local().unwrap(),
            "fwd_test".to_string(),
            "test-session".to_string(),
            PublicForwardPortProtocol::Tcp,
            Duration::from_secs(30),
            PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            Some(listen_tunnel.clone()),
        ));
        let cancel = CancellationToken::new();
        let runtime = ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local().unwrap(),
            connect_side: SideHandle::local().unwrap(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            max_active_tcp_streams_per_forward: 256,
            max_pending_tcp_bytes_per_stream: 256 * 1024,
            max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
            max_udp_peers_per_forward: 256,
            max_tunnel_queued_bytes: PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            max_reconnecting_forwards: 16,
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel.clone(),
            cancel,
        };

        let epoch_runtime = runtime.clone();
        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_tcp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
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

        let listen_session = Arc::new(ListenSessionControl::new_for_test(
            SideHandle::local().unwrap(),
            "fwd_test".to_string(),
            "test-session".to_string(),
            PublicForwardPortProtocol::Tcp,
            Duration::from_secs(30),
            PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            Some(listen_tunnel.clone()),
        ));
        let cancel = CancellationToken::new();
        let runtime = ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local().unwrap(),
            connect_side: SideHandle::local().unwrap(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            max_active_tcp_streams_per_forward: 256,
            max_pending_tcp_bytes_per_stream: 256 * 1024,
            max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
            max_udp_peers_per_forward: 256,
            max_tunnel_queued_bytes: PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            max_reconnecting_forwards: 16,
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel.clone(),
            cancel,
        };

        let epoch_runtime = runtime.clone();
        let epoch = tokio::spawn({
            let listen_tunnel = listen_tunnel.clone();
            let connect_tunnel = connect_tunnel.clone();
            async move { run_tcp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
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
            async move { run_tcp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
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
                let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
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
            async move { run_tcp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
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
                let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
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
            async move { run_tcp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
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
                let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
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
            async move { run_tcp_forward_epoch(&epoch_runtime, listen_tunnel, connect_tunnel).await }
        });

        write_frame(
            &mut listen_daemon_side,
            &Frame {
                frame_type: FrameType::ForwardDrop,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&ForwardDropMeta {
                    kind: ForwardDropKind::TcpStream,
                    count: 2,
                    reason: RpcErrorCode::PortTunnelLimitExceeded
                        .wire_value()
                        .to_string(),
                    message: Some("port tunnel active tcp stream limit reached".to_string()),
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let entries = runtime.store.list(&filter_one(&runtime.forward_id)).await;
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
        let listen_session = Arc::new(ListenSessionControl::new_for_test(
            SideHandle::local().unwrap(),
            "fwd_test".to_string(),
            "test-session".to_string(),
            PublicForwardPortProtocol::Tcp,
            Duration::from_secs(30),
            PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            Some(listen_tunnel),
        ));
        ForwardRuntime {
            forward_id: "fwd_test".to_string(),
            listen_side: SideHandle::local().unwrap(),
            connect_side: SideHandle::local().unwrap(),
            protocol: PublicForwardPortProtocol::Tcp,
            connect_endpoint: "127.0.0.1:1".to_string(),
            max_active_tcp_streams_per_forward: 256,
            max_pending_tcp_bytes_per_stream: 256 * 1024,
            max_pending_tcp_bytes_per_forward: 2 * 1024 * 1024,
            max_udp_peers_per_forward: 256,
            max_tunnel_queued_bytes: PortTunnel::DEFAULT_MAX_QUEUED_BYTES,
            max_reconnecting_forwards: 16,
            store: Default::default(),
            listen_session,
            initial_connect_tunnel: connect_tunnel,
            cancel: CancellationToken::new(),
        }
    }

    fn filter_one(forward_id: &str) -> super::super::store::PortForwardFilter {
        super::super::store::PortForwardFilter {
            listen_side: None,
            connect_side: None,
            forward_ids: vec![forward_id.to_string()],
        }
    }

    fn test_record(
        runtime: &ForwardRuntime,
        listen_endpoint: &str,
    ) -> super::super::store::PortForwardRecord {
        super::super::store::PortForwardRecord::new(
            ForwardPortEntry::new_open(
                runtime.forward_id.clone(),
                runtime.listen_side.name().to_string(),
                listen_endpoint.to_string(),
                runtime.connect_side.name().to_string(),
                runtime.connect_endpoint.clone(),
                runtime.protocol,
                ForwardPortLimitSummary {
                    max_active_tcp_streams: runtime.max_active_tcp_streams_per_forward,
                    max_udp_peers: runtime.max_udp_peers_per_forward as u64,
                    max_pending_tcp_bytes_per_stream: runtime.max_pending_tcp_bytes_per_stream
                        as u64,
                    max_pending_tcp_bytes_per_forward: runtime.max_pending_tcp_bytes_per_forward
                        as u64,
                    max_tunnel_queued_bytes: runtime.max_tunnel_queued_bytes as u64,
                    max_reconnecting_forwards: runtime.max_reconnecting_forwards,
                },
            ),
            runtime.listen_session.clone(),
            runtime.cancel.clone(),
        )
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
